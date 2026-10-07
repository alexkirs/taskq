"""GitHub as the queue's store: `Github` speaks the GitLab REST subset taskq uses, through `gh api`."""
import base64
from datetime import datetime, timezone
import json
import re
from urllib.parse import parse_qs, quote

import taskq as core


class Github:
    """The store protocol on GitHub REST through `gh api`. An issue's `number` is `iid`, `body` is `description`,
    `open` is `opened`, labels come back as names, assignees as `{id, username}`; a PUT with add/remove labels
    sends the full set built from a GET made right before it, never from a list read earlier in the process. The lock is the ref
    `refs/taskq/lock/<N>` on a blob holding the time: a second POST is 422 for any user (atomic between users,
    owner's rule 2026-10-06), it is no branch so no CI runs, and anyone may remove it — the claim names the
    holder. Issue lists come from GraphQL: the REST list lags a new issue by up to half a minute, GraphQL shows
    it at once (measured live 2026-10-06). Lists are read whole on page 1, later pages are empty. Dependencies
    have no links here (`deps` in the block is the source of truth). The board is the Projects v2 project linked to
    the repository and titled [github] board (default: the repository name), with Status options STATES: the store adds every new issue and sets Status in the same PUT that moves
    the q-* label, archives the item on close, and answers `board` routes (`GET board`, `POST board` to create,
    `GET board/items`, `PUT board/items/N`). Without the token scope `project` there is no board, silently."""
    LIST = ('query($owner: String!, $name: String!, $states: [IssueState!], $labels: [String!], $filter: IssueFilters, $after: String) {'
            ' repository(owner: $owner, name: $name) { issues(states: $states, labels: $labels, filterBy: $filter, first: 100, after: $after,'
            ' orderBy: {field: CREATED_AT, direction: DESC}) { pageInfo { hasNextPage endCursor } nodes { number id title body state url'
            ' createdAt updatedAt labels(first: 100) { nodes { name } } assignees(first: 10) { nodes { databaseId login } }'
            ' milestone { number } comments { totalCount } author { login ... on User { databaseId } } authorAssociation } } } }')
    PROJECT = ('id number title url field(name: "Status") { ... on ProjectV2SingleSelectField { id options { id name } } }')
    # The repository's linked projects, not the owner's: an owner-level project of the same title is another queue's.
    FIND = ('query($owner: String!, $name: String!, $board: String!) { repository(owner: $owner, name: $name) { id owner { id }'
            ' projectsV2(first: 20, query: $board) { nodes { %s } } } }' % PROJECT)
    # Cards are read from the open issues' side: `ProjectV2.items` of a new project stayed empty for minutes while
    # `Issue.projectItems` showed the cards at once (measured live 2026-10-06).
    # `board` rides in the same query: the cards of issues closed outside `close` (#152). Its lag on a new project only
    # delays an archive to a later tick.
    ITEMS = ('query($owner: String!, $name: String!, $after: String, $project: ID!) { repository(owner: $owner, name: $name) {'
             ' issues(states: OPEN, first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { number labels(first: 100) { nodes { name } }'
             ' timelineItems(itemTypes: [LABELED_EVENT], last: 1) { nodes { ... on LabeledEvent { createdAt } } }'
             ' projectItems(first: 10, includeArchived: false) { nodes { id updatedAt project { id }'
             ' fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue { name } } } } } } }'
             ' board: node(id: $project) { ... on ProjectV2 { items(first: 100) { nodes { id isArchived content { ... on Issue { number state } } } } } } }')

    def __init__(self, repo, host=None, board=None):
        self.repo, self.host, self.labels, self.nodes = repo, host, {}, {}
        self.title = board or repo.split('/')[-1]  # the board's title: [github] board, else the repository name  # by issue number: label names last read, GraphQL id
        self.board, self.items = None, {}  # the project once looked up in this process (False: none); item ids by issue number

    def run(self, method, path, body=None):
        own = path.startswith(('user', 'graphql', 'repos/'))
        command = ['gh', 'api', '-X', method, path if own else f'repos/{self.repo}/{path}'] + (['--hostname', self.host] if self.host else [])
        if body is not None:
            command += ['--input', '-']
        return core.cli_api(command, body, f'GitHub {method} {path}')

    def all(self, path):
        found, page = [], 1
        while True:
            batch = self.run('GET', f'{path}{"&" if "?" in path else "?"}per_page=100&page={page}')
            found += batch
            if len(batch) < 100:
                return found
            page += 1

    def node(self, item):
        """A GraphQL issue node in the REST shape `issue` reads."""
        return {'number': item['number'], 'node_id': item['id'], 'title': item['title'], 'body': item['body'],
                'state': item['state'].lower(), 'html_url': item['url'], 'created_at': item['createdAt'], 'updated_at': item['updatedAt'],
                'labels': item['labels']['nodes'], 'assignees': [{'id': each['databaseId'], 'login': each['login']} for each in item['assignees']['nodes']],
                'milestone': item['milestone'], 'comments': item['comments']['totalCount'],
                'user': {'id': (item['author'] or {}).get('databaseId'), 'login': (item['author'] or {}).get('login')},
                'author_association': item['authorAssociation']}

    def listed(self, query):
        """Every issue the GitLab-style `query` names, through GraphQL."""
        states = {'opened': ['OPEN'], 'closed': ['CLOSED'], 'all': None}[query.get('state', 'opened')]
        filters = {key: query[name] for name, key in (('assignee', 'assignee'), ('creator', 'createdBy'), ('milestone', 'milestoneNumber'), ('updated_after', 'since')) if name in query}
        found, after = [], None
        while True:
            variables = {'owner': self.repo.split('/')[0], 'name': self.repo.split('/')[1], 'states': states,
                         'labels': query['labels'].split(',') if query.get('labels') else None, 'filter': filters or None, 'after': after}
            page = self.run('POST', 'graphql', {'query': self.LIST, 'variables': variables})['data']['repository']['issues']
            wanted = set(variables['labels'] or ())  # GraphQL `labels` is any-of; GitLab's `labels=` is all-of
            found += [self.issue(self.node(item)) for item in page['nodes'] if wanted <= {label['name'] for label in item['labels']['nodes']}]
            if not page['pageInfo']['hasNextPage']:
                return found
            after = page['pageInfo']['endCursor']

    def graphql(self, query, **variables):
        return self.run('POST', 'graphql', {'query': query, 'variables': variables})['data']

    def project(self, create=False):
        """The board: the project titled `self.title` linked to the repository, {id, url, field, options by name, number}; None without one, False without the scope `project`.
        `create` makes it once, linked to the repository, with Status options exactly STATES."""
        if self.board is None or (create and not self.board):
            owner, name = self.repo.split('/')
            try:
                repository = self.graphql(self.FIND, owner=owner, name=name, board=self.title)['repository']
            except SystemExit as error:
                if 'scope' not in str(error).lower():
                    raise
                self.board = False
                return False
            found = next((item for item in repository['projectsV2']['nodes'] if item['title'] == self.title), None)
            if not found and create:
                found = self.graphql('mutation($owner: ID!, $title: String!, $repo: ID!) { createProjectV2(input: {ownerId: $owner,'
                                     ' title: $title, repositoryId: $repo}) { projectV2 { %s } } }' % self.PROJECT,
                                     owner=repository['owner']['id'], title=self.title, repo=repository['id'])['createProjectV2']['projectV2']
            if found and create:
                # The project's own workflows move cards and close issues (Status Done closes the issue, a closed or added
                # item gets a Status): taskq alone writes Status. The API can only delete them (checked live 2026-10-06).
                workflows = self.graphql('query($id: ID!) { node(id: $id) { ... on ProjectV2 { workflows(first: 20) { nodes { id } } } } }',
                                         id=found['id'])['node']['workflows']['nodes']
                for workflow in workflows:
                    self.graphql('mutation($id: ID!) { deleteProjectV2Workflow(input: {workflowId: $id}) { deletedWorkflowId } }', id=workflow['id'])
            if found and create and [option['name'] for option in (found['field'] or {}).get('options', [])] != list(core.STATES):
                # New options, not renamed ones: an old option's id may still be bound to something (Todo/In Progress/Done go).
                options = [{'name': state, 'color': 'GRAY', 'description': ''} for state in core.STATES]
                if found['field']:
                    found['field'] = self.graphql('mutation($field: ID!, $options: [ProjectV2SingleSelectFieldOptionInput!]) {'
                                                  ' updateProjectV2Field(input: {fieldId: $field, singleSelectOptions: $options}) {'
                                                  ' projectV2Field { ... on ProjectV2SingleSelectField { id options { id name } } } } }',
                                                  field=found['field']['id'], options=options)['updateProjectV2Field']['projectV2Field']
                else:
                    found['field'] = self.graphql('mutation($project: ID!, $options: [ProjectV2SingleSelectFieldOptionInput!]) {'
                                                  ' createProjectV2Field(input: {projectId: $project, dataType: SINGLE_SELECT, name: "Status",'
                                                  ' singleSelectOptions: $options}) { projectV2Field { ... on ProjectV2SingleSelectField {'
                                                  ' id options { id name } } } } }',
                                                  project=found['id'], options=options)['createProjectV2Field']['projectV2Field']
            self.board = found and {'id': found['id'], 'url': found['url'], 'field': (found['field'] or {}).get('id'),
                                    'options': {option['name']: option['id'] for option in (found['field'] or {}).get('options', [])},
                                    'number': found['number']}
        return self.board  # False: the token lacks the scope `project`

    def cards(self):
        """Status by number of every open issue with a card on the board (None: the card has no Status). A Status
        unlike the label is the owner's intent only when the card changed after the issue's last label event;
        otherwise a card sync failed after the label moved (#45): the card is put back to the label here."""
        board, found, after = self.project(), {}, None
        while board:
            owner, name = self.repo.split('/')
            data = self.graphql(self.ITEMS, owner=owner, name=name, after=after, project=board['id'])
            page = data['repository']['issues']
            if after is None:  # ponytail: the first 100 cards only; read `items` pages if a board ever holds more
                for item in data['board']['items']['nodes']:
                    content = item['content'] or {}
                    if content.get('state') == 'CLOSED' and not item['isArchived']:
                        print(f'Board card of #{content["number"]} archived: its issue is closed.')
                        self.archive(item['id'])
            for issue in page['nodes']:
                number, label = issue['number'], self.state(node['name'] for node in issue['labels']['nodes'])
                item = next((item for item in issue['projectItems']['nodes'] if item['project']['id'] == board['id']), None)
                if not item:
                    if label:  # the card step of `add` failed: the task gets its card now
                        print(f'Board card of #{number} added in {label}: it had none.')
                        self.sync(number, label)
                        found[number] = label
                    continue
                self.items[number] = item['id']
                status = (item['fieldValueByName'] or {}).get('name')
                labeled = max((event['createdAt'] for event in issue['timelineItems']['nodes']), default='')
                if status and label and status != label and item['updatedAt'] <= labeled:
                    print(f'Board card of #{number} put back to {label}: its last card sync failed.')
                    self.sync(number, label)
                    status = label
                found[number] = status
            if not page['pageInfo']['hasNextPage']:
                break
            after = page['pageInfo']['endCursor']
        return found

    def card(self, number, state):
        """Put the issue's card in the column `state`; None archives it (a closed task leaves the board)."""
        board = self.project()
        if not board or (state and state not in board['options']):
            return
        if number not in self.items:
            node = self.nodes.get(number) or self.run('GET', f'issues/{number}')['node_id']
            self.items[number] = self.graphql('mutation($project: ID!, $node: ID!) { addProjectV2ItemById(input: {projectId: $project,'
                                              ' contentId: $node}) { item { id } } }', project=board['id'], node=node)['addProjectV2ItemById']['item']['id']
        if state is None:
            self.archive(self.items.pop(number))
            return
        self.graphql('mutation($project: ID!, $item: ID!, $field: ID!, $option: String!) { updateProjectV2ItemFieldValue(input: {'
                     ' projectId: $project, itemId: $item, fieldId: $field, value: {singleSelectOptionId: $option}}) { projectV2Item { id } } }',
                     project=board['id'], item=self.items[number], field=board['field'], option=board['options'][state])

    def archive(self, item):
        self.graphql('mutation($project: ID!, $item: ID!) { archiveProjectV2Item(input: {projectId: $project, itemId: $item}) {'
                     ' item { id } } }', project=self.board['id'], item=item)

    def sync(self, number, state):
        """`card` as best effort: the label is the queue's state, a failed card is put back by the next `cards`."""
        try:
            self.card(number, state)
        except SystemExit as error:
            print(f'Board card of #{number} not updated: {error}. The next tick puts it back.')

    @staticmethod
    def state(names):
        return next((name[len(core.PREFIX):] for name in names if name.startswith(core.PREFIX)), None)

    def issue(self, item):
        self.labels[item['number']] = [label['name'] for label in item['labels']]
        self.nodes[item['number']] = item['node_id']
        return {'iid': item['number'], 'title': item['title'], 'description': item.get('body') or '',
                'labels': self.labels[item['number']], 'state': 'opened' if item['state'] == 'open' else 'closed',
                'assignees': [{'id': each['id'], 'username': each['login']} for each in item.get('assignees', [])],
                'milestone_id': (item.get('milestone') or {}).get('number'), 'web_url': item['html_url'],
                'created_at': item['created_at'], 'updated_at': item['updated_at'], 'comments': item.get('comments', 0),
                'author': {'id': (item.get('user') or {}).get('id'), 'username': (item.get('user') or {}).get('login')},
                'author_association': item.get('author_association', '')}

    @staticmethod
    def comment(item):
        return {'id': item['id'], 'body': item['body'], 'created_at': item['created_at'], 'system': False,
                'author': {'id': item['user']['id'], 'username': item['user']['login']}, 'author_association': item.get('author_association', '')}

    def body(self, body, iid=None):
        out = {key: body[key] for key in ('title',) if key in body}
        if 'description' in body:
            out['body'] = body['description']
        if 'labels' in body:
            out['labels'] = body['labels'].split(',')
        if 'add_labels' in body or 'remove_labels' in body:
            have = self.issue(self.run('GET', f'issues/{iid}'))['labels']  # a list read is stale by now: another session may have moved it
            drop = body.get('remove_labels', '').split(',')
            out['labels'] = [name for name in have if name not in drop] + [name for name in body.get('add_labels', '').split(',') if name and name not in have]
        if 'assignee_ids' in body:
            out['assignees'] = [self.run('GET', f'user/{uid}')['login'] for uid in body['assignee_ids']]
        if 'milestone_id' in body:
            out['milestone'] = body['milestone_id']
        if body.get('state_event') == 'close':
            out['state'] = 'closed'
        return out

    def __call__(self, method, path, body=None):
        query = {key: value[0] for key, value in parse_qs(path.partition('?')[2]).items()}
        later = int(query.get('page', 1)) > 1
        if path == '/user':
            return {'id': self.run('GET', 'user')['id']}
        if path == 'repository':
            return self.run('GET', f'repos/{self.repo}')
        if path.startswith('milestones'):
            return [{'id': item['number'], 'title': item['title']} for item in self.all('milestones?state=open')]
        if path.startswith('labels'):
            if method == 'DELETE':
                return self.run('DELETE', 'labels/' + quote(path.split('/', 1)[1], safe=''))
            if method == 'POST':
                return self.run('POST', 'labels', {'name': body['name'], 'color': body['color'].lstrip('#')})
            return [] if later else self.all('labels')
        if path == 'board':
            return self.project(create=method == 'POST')
        if path == 'board/items':
            return self.cards()
        if path.startswith('board/items/'):
            return self.card(int(path.rsplit('/', 1)[1]), body['status'])
        if path == 'leases':  # #145: the refs refs/taskq/coordinator/<key> the #44 lease left; doctor --fix deletes them
            refs = [item['ref'] for item in self.run('GET', 'git/matching-refs/taskq/coordinator/')]
            if method == 'DELETE':
                for ref in refs:
                    self.run('DELETE', 'git/' + ref)
            return refs
        if path.startswith('boards'):
            core.fail('GitHub has no GitLab board: the Projects v2 board is `board`')
        if method == 'POST' and path == 'issues':
            created = self.issue(self.run('POST', 'issues', self.body(body)))
            if self.state(created['labels']):
                self.sync(created['iid'], self.state(created['labels']))
            return created
        if method == 'GET' and path.startswith('issues?'):
            if 'my_reaction_emoji' in query:  # every locked issue: the lock refs name them
                numbers = [int(item['ref'].rsplit('/', 1)[1]) for item in self.run('GET', 'git/matching-refs/taskq/lock/')]
                found = []
                for number in numbers:
                    try:
                        found.append(self.issue(self.run('GET', f'issues/{number}')))
                    except SystemExit as error:
                        if not core.gone(error):
                            raise
                        self.run('DELETE', f'git/refs/taskq/lock/{number}')  # its issue was deleted: the lock guards nothing
                return [item for item in found if query.get('state', 'opened') in ('all', item['state'])]
            return [] if later else self.listed(query)
        iid = int(re.match(r'issues/(\d+)', path)[1])
        rest = path[len(f'issues/{iid}'):].partition('?')[0]
        if rest == '/award_emoji':
            if method == 'POST':
                now = datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%S.%f')[:-3] + 'Z'
                blob = self.run('POST', 'git/blobs', {'content': json.dumps({'created_at': now})})
                self.run('POST', 'git/refs', {'ref': f'refs/taskq/lock/{iid}', 'sha': blob['sha']})
                return {'id': iid, 'name': core.LOCK, 'user': {'id': 0}, 'created_at': now}
            try:
                ref = self.run('GET', f'git/ref/taskq/lock/{iid}')
            except SystemExit as error:
                if core.gone(error):
                    return []
                raise
            content = json.loads(base64.b64decode(self.run('GET', f'git/blobs/{ref["object"]["sha"]}')['content']))
            return [{'id': iid, 'name': core.LOCK, 'user': {'id': self('GET', '/user')['id']}, 'created_at': content['created_at']}]
        if rest.startswith('/award_emoji/') and method == 'DELETE':
            return self.run('DELETE', f'git/refs/taskq/lock/{iid}')
        if rest == '/resource_label_events':
            return [] if later else [{'action': 'add', 'label': {'name': item['label']['name']}, 'created_at': item['created_at']}
                                     for item in self.all(f'issues/{iid}/events') if item['event'] == 'labeled']
        if rest == '/links':
            return [] if method == 'GET' else None
        if rest == '/notes':
            if method == 'POST':
                return self.comment(self.run('POST', f'issues/{iid}/comments', {'body': body['body']}))
            if query.get('sort') == 'desc':  # GitHub lists comments oldest first only: read from the last page back
                page, want, found = max(1, -(-self.run('GET', f'issues/{iid}')['comments'] // 100)), int(query.get('per_page', 100)), []
                while page >= 1 and len(found) < want:
                    found, page = self.run('GET', f'issues/{iid}/comments?per_page=100&page={page}') + found, page - 1
                return [self.comment(item) for item in found[::-1][:want]]
            return [] if later else [self.comment(item) for item in self.all(f'issues/{iid}/comments')]
        if rest.startswith('/notes/') and method == 'DELETE':
            return self.run('DELETE', f'issues/comments/{rest.rsplit("/", 1)[1]}')
        if method == 'DELETE':
            node = self.nodes.get(iid) or self.run('GET', f'issues/{iid}')['node_id']
            return self.run('POST', 'graphql', {'query': 'mutation($id: ID!) { deleteIssue(input: {issueId: $id}) { clientMutationId } }',
                                                'variables': {'id': node}})
        if method == 'PUT':
            patch = self.body(body, iid)
            before = self.state(self.labels.get(iid, ()))
            changed = self.issue(self.run('PATCH', f'issues/{iid}', patch))
            if changed['state'] == 'closed' and body.get('state_event') == 'close':
                self.sync(iid, None)
            elif 'labels' in patch and self.state(changed['labels']) and self.state(changed['labels']) != before:
                self.sync(iid, self.state(changed['labels']))
            return changed
        return self.issue(self.run('GET', f'issues/{iid}'))
