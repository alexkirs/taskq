"""The six operations a taskq runtime supports."""
from importlib import import_module


def get(name, **options):
    return import_module(f'taskq.runtimes.{name}').Adapter(**options)
