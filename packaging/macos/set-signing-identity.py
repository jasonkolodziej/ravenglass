"""Put APPLE_SIGNING_IDENTITY into the packager's macOS config, for one CI run (never committed).

cargo-packager reads the signing identity only from its config, so the release workflow calls
`python3 packaging/macos/set-signing-identity.py app/Cargo.toml` when the secret exists. The value
comes from the environment and is written as a TOML string, so it cannot break the file.
"""
import json, os, sys

path = sys.argv[1]
identity = os.environ.get('APPLE_SIGNING_IDENTITY', '')
if not identity:
    sys.exit('APPLE_SIGNING_IDENTITY is not set')
header = '[package.metadata.packager.macos]\n'
text = open(path, encoding='utf-8').read()
if header not in text:
    sys.exit(f'{path}: no {header.strip()} table')
# A JSON string is a valid TOML basic string.
text = text.replace(header, header + f'signing-identity = {json.dumps(identity)}\n', 1)
open(path, 'w', encoding='utf-8').write(text)
print('macOS signing identity set for this build')
