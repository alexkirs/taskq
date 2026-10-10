"""#538 dormant selection proposal; no board, runtime or production imports."""


def eligible(assignees, authenticated_login, strict=False):
    """Native membership for assigned tasks; proposed shared unassigned intake."""
    if not assignees:
        return not strict
    return (isinstance(authenticated_login, str) and bool(authenticated_login.strip())
            and authenticated_login in assignees)
