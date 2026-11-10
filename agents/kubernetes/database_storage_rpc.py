"""Root-only forced SSH command for the private PostgreSQL storage helper."""
import json
import os
import sys
from database_storage import validate
from storage_client import request

SOCKET = '/run/webdock-postgres-storage/control.sock'
ACTIONS = {'database.capability', 'database.ensure', 'database.state',
           'database.retained', 'database.purge'}
REJECTED = b'{"ok":false,"value":null}\n'


def reject():
    sys.stdout.buffer.write(REJECTED)
    return 1


def main():
    try:
        if os.geteuid() != 0:raise ValueError('Root required')
        if os.environ.get('SPITZLI_STORAGE_SOCKET', SOCKET) != SOCKET:
            raise ValueError('Socket overrides are not supported')
        raw = sys.stdin.buffer.readline(4097)
        if len(raw) > 4096 or not raw.endswith(b'\n'):raise ValueError('Invalid request bounds')
        message = json.loads(raw)
        if not isinstance(message, dict) or set(message) != {'action', 'value'}:
            raise ValueError('Invalid request')
        action, value = message['action'], message['value']
        if action not in ACTIONS:raise ValueError('Invalid action')
        if action == 'database.capability':
            if not isinstance(value, list) or len(value) > 200 or any(
                not isinstance(node, str) or len(node) > 253 for node in value
            ):raise ValueError('Invalid nodes')
        else:validate(value)
    except Exception:return reject()
    try:
        value = request(action, value, socket_path=SOCKET)
    # Empty EOF means unavailable; nonempty rejection/malformed data stays a hard failure.
    except OSError:return 1
    except Exception:return reject()
    try:
        output = (json.dumps({'ok': True, 'value': value}, allow_nan=False) + '\n').encode()
        if len(output) > 8192:raise ValueError('Invalid response bounds')
    except Exception:return reject()
    sys.stdout.buffer.write(output)
    return 0


if __name__ == '__main__':raise SystemExit(main())
