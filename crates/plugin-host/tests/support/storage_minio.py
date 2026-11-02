"""One fresh bucket only; private presign fixture never writes credentials or URLs to output."""
import datetime
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import stat
import sys
import urllib.parse
import urllib.request


def main():
    action, fixture_path = sys.argv[1:]
    path = Path(fixture_path)
    assert path.is_absolute()
    root = Path(__file__).resolve().parents[4]
    credential_path = root / '.dev-local/minio.env'
    info = credential_path.lstat()
    assert stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_mode & 0o077 == 0
    secret = dict(line.split('=', 1) for line in credential_path.read_text().splitlines() if '=' in line)
    access, key = secret['MINIO_ROOT_USER'], secret['MINIO_ROOT_PASSWORD']
    origin = 'http://127.0.0.1:55900'
    now = datetime.datetime.now(datetime.timezone.utc)
    timestamp, day = now.strftime('%Y%m%dT%H%M%SZ'), now.strftime('%Y%m%d')
    scope = f'{day}/us-east-1/s3/aws4_request'

    def sign(method, target):
        query = urllib.parse.urlencode(sorted({
            'X-Amz-Algorithm': 'AWS4-HMAC-SHA256', 'X-Amz-Credential': f'{access}/{scope}',
            'X-Amz-Date': timestamp, 'X-Amz-Expires': '240', 'X-Amz-SignedHeaders': 'host',
        }.items()), quote_via=urllib.parse.quote, safe='~')
        canonical = f'{method}\n{target}\n{query}\nhost:127.0.0.1:55900\n\nhost\nUNSIGNED-PAYLOAD'
        to_sign = f'AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}'
        derived = ('AWS4' + key).encode()
        for part in (day, 'us-east-1', 's3', 'aws4_request'):
            derived = hmac.new(derived, part.encode(), hashlib.sha256).digest()
        signature = hmac.new(derived, to_sign.encode(), hashlib.sha256).hexdigest()
        return origin + target + '?' + query + '&X-Amz-Signature=' + signature

    def request(method, target):
        data = b'' if method == 'PUT' else None
        with urllib.request.urlopen(urllib.request.Request(sign(method, target), data=data, method=method), timeout=5) as response:
            assert response.status in (200, 204)

    if action == 'prepare':
        # The caller chooses a fresh canonical UUID; no preexisting bucket can enter this fixture.
        bucket = 'wd-transfer-kernel-' + path.stem
        assert re.fullmatch(r'wd-transfer-kernel-[0-9a-f]{32}', bucket)
        request('PUT', '/' + bucket)
        payload = {'bucket': bucket, 'origin': origin, 'put': sign('PUT', '/' + bucket + '/object'),
                   'get': sign('GET', '/' + bucket + '/object'), 'expiresAt': int(now.timestamp()) + 240}
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'w') as output:
            json.dump(payload, output)
    elif action == 'cleanup':
        payload = json.loads(path.read_text())
        bucket = payload['bucket']
        assert bucket == 'wd-transfer-kernel-' + path.stem
        assert re.fullmatch(r'wd-transfer-kernel-[0-9a-f]{32}', bucket)
        request('DELETE', '/' + bucket + '/object')
        request('DELETE', '/' + bucket)
        path.unlink()
    else:
        raise ValueError('unknown fixture action')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('owned MinIO fixture failed (private details withheld)', file=sys.stderr)
        sys.exit(1)
