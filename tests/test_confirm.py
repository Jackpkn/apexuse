import pytest
from handsmcp.confirm import ConfirmationStore


def test_confirmation_is_bound_to_action_and_single_use():
    store = ConfirmationStore(ttl_seconds=120, clock=lambda: 10)
    token = store.issue({'action': 'click', 'ref': 'r1'})
    assert store.consume(token) == {'action': 'click', 'ref': 'r1'}
    assert store.consume(token) is None


def test_expired_confirmation_is_not_valid():
    now = [0]
    store = ConfirmationStore(ttl_seconds=120, clock=lambda: now[0])
    token = store.issue({'action': 'click', 'ref': 'r1'})
    now[0] = 120
    assert store.consume(token) is None


def test_token_cannot_be_changed_by_caller_mutation():
    store = ConfirmationStore(ttl_seconds=120, clock=lambda: 0)
    request = {'action': 'click', 'target': {'ref': 'r1'}}
    token = store.issue(request)
    request['target']['ref'] = 'r2'
    assert store.consume(token)['target']['ref'] == 'r1'
