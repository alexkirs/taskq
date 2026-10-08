"""GitHub as the queue's store: `Github` speaks the GitLab REST subset taskq uses, through `gh api`."""
import re
from urllib.parse import parse_qs, quote

import taskq as core


class Github:
    """The store protocol on GitHub REST through `gh api`. An issue's `number` is `iid`, `body` is `description`,
    `open` is `opened`, labels come back as names, assignees as `{id, username}`; a PUT with add/remove labels
    sends the full set built from a GET made right before it, never from a list read earlier in the process.
    Issue lists come from GraphQL: the REST list lags a new issue by up to half a minute, GraphQL shows
    it at once (measured live 2026-10-06). Lists are read whole on page 1, later pages are empty. Dependencies
    have no links here (`deps` in the block is the source of truth). The board is the issue list filtered by the
    q-* labels (`core.issue_board`, owner 2026-10-08): no Projects v2 copy."""
    LIST = ('query($owner: String!, $name: String!, $states: [IssueState!], $labels: [String!], $filter: IssueFilters, $after: String) {'
            ' viewer { databaseId }'
            ' repository(owner: $owner, name: $name) { issues(states: $states, labels: $labels, filterBy: $filter, first: 100, after: $after,'
            ' orderBy: {field: CREATED_AT, direction: DESC}) { pageInfo { hasNextPage endCursor } nodes { number id title body state url'
            ' createdAt updatedAt labels(first: 100) { nodes { name } } assignees(first: 10) { nodes { databaseId login } }'
            ' milestone { number } comments { totalCount } author { login ... on User { databaseId } } authorAssociation } } } }')
    def __init__(self, repo, host=None):
        self.repo, self.host, self.nodes, self.viewer = repo, host, {}, None  # nodes: GraphQL id by issue number

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
            data = self.run('POST', 'graphql', {'query': self.LIST, 'variables': variables})['data']
            self.viewer = (data.get('viewer') or {}).get('databaseId') or self.viewer
            page = data['repository']['issues']
            wanted = set(variables['labels'] or ())  # GraphQL `labels` is any-of; GitLab's `labels=` is all-of
            found += [self.issue(self.node(item)) for item in page['nodes'] if wanted <= {label['name'] for label in item['labels']['nodes']}]
            if not page['pageInfo']['hasNextPage']:
                return found
            after = page['pageInfo']['endCursor']

    def graphql(self, query, **variables):
        return self.run('POST', 'graphql', {'query': query, 'variables': variables})['data']

    def issue(self, item):
        self.nodes[item['number']] = item['node_id']
        return {'iid': item['number'], 'title': item['title'], 'description': item.get('body') or '',
                'labels': [label['name'] for label in item['labels']], 'state': 'opened' if item['state'] == 'open' else 'closed',
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
            return {'id': self.viewer} if self.viewer is not None else {'id': self.run('GET', 'user')['id']}
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
        if path.startswith('boards'):
            core.fail('GitHub has no GitLab board: its board is the issue list filtered by the q-* labels')
        if method == 'POST' and path == 'issues':
            return self.issue(self.run('POST', 'issues', self.body(body)))
        if method == 'GET' and path.startswith('issues?'):
            return [] if later else self.listed(query)
        iid = int(re.match(r'issues/(\d+)', path)[1])
        rest = path[len(f'issues/{iid}'):].partition('?')[0]
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
            return self.issue(self.run('PATCH', f'issues/{iid}', self.body(body, iid)))
        return self.issue(self.run('GET', f'issues/{iid}'))
