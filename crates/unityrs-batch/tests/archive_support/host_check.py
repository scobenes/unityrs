"""Host-only real encoder check; does not replace full stream/accounting gates.

Compile encoding.rs with rustc --test, then pass the test executable here.
The small adapter substitutes only Buffer's allocation implementation.
"""
import hashlib
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools' / 'parity'))
import child  # noqa: E402
import scratch  # noqa: E402
from check import check  # noqa: E402


def run(binary):
    with scratch.folder('archive-encoder-') as folder:
        env = dict(os.environ, UNITYRS_ARCHIVE_ORACLE_DIR=str(folder))
        child.run([str(Path(binary).resolve()), '--exact', 'emit_for_python'],
                  env=env, check=True, timeout=60)
        for path in sorted(folder.glob('*.zip')):
            if path.name.startswith('offset-'):
                origin = int(path.stem.split('-')[1])
                members = [['a', '78'], ['b', '797a']]
            else:
                origin = 0
                members = {'count': int(path.stem.split('-')[1])}
            description = folder / 'members.json'
            summary = folder / 'summary.json'
            description.write_text(json.dumps(members))
            data = path.read_bytes()
            summary.write_text(json.dumps({
                'size': origin + len(data), 'emitted_size': len(data),
                'md5': hashlib.md5(data).hexdigest(),
                'etag': hashlib.md5(data).hexdigest(),
            }))
            check(path, description, origin, summary)
            print(f'ENCODER_ORACLE {path.name}: exact bytes and every member read')


if __name__ == '__main__':
    run(sys.argv[1])
