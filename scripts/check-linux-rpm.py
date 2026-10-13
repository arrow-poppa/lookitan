"""Check the actual RPM desktop entry used by KWin's executable-path authorization."""
import gzip
from pathlib import Path
import struct


def exact(stream, size):
    data = stream.read(size)
    if len(data) != size:
        raise ValueError('Truncated RPM/CPIO')
    return data


def skip(stream, size):
    while size:
        chunk = exact(stream, min(size, 1024 * 1024))
        size -= len(chunk)


def rpm_header(stream):
    header = exact(stream, 16)
    if header[:4] != b'\x8e\xad\xe8\x01':
        raise ValueError('Invalid RPM header')
    entries, size = struct.unpack('>II', header[8:])
    skip(stream, entries * 16 + size)


def validate(path):
    desktop = None
    binary = False
    with path.open('rb') as rpm:
        if exact(rpm, 96)[:4] != b'\xed\xab\xee\xdb':
            raise ValueError('Invalid RPM lead')
        rpm_header(rpm)
        skip(rpm, (-rpm.tell()) % 8)
        rpm_header(rpm)
        with gzip.GzipFile(fileobj=rpm) as cpio:
            while True:
                header = exact(cpio, 110)
                if header[:6] not in (b'070701', b'070702'):
                    raise ValueError('Expected newc CPIO')
                fields = [int(header[6 + i * 8:14 + i * 8], 16) for i in range(13)]
                size, name_size = fields[6], fields[11]
                name = exact(cpio, name_size).rstrip(b'\0').decode('utf-8').removeprefix('./')
                skip(cpio, (-(110 + name_size)) % 4)
                if name == 'TRAILER!!!':
                    break
                if name == 'usr/share/applications/Lookitan.desktop':
                    if size > 65536:
                        raise ValueError('Oversized desktop entry')
                    desktop = exact(cpio, size).decode('utf-8')
                else:
                    if name == 'usr/bin/lookitan':
                        binary = True
                    skip(cpio, size)
                skip(cpio, (-size) % 4)
    if not binary or desktop is None:
        raise ValueError('RPM missing binary or desktop entry')
    required = {'Exec=/usr/bin/lookitan', 'Exec=/usr/bin/lookitan --scan',
                'X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2'}
    if not required.issubset(set(desktop.splitlines())):
        raise ValueError('RPM desktop entry cannot authorize the installed executable for KWin capture')
    print(f'{path.name}: installed binary and KWin capture declaration match')


if __name__ == '__main__':
    import sys
    paths = [Path(arg) for arg in sys.argv[1:]] or list(Path('src-tauri/target/release/bundle/rpm').glob('*.rpm'))
    if not paths:
        raise SystemExit('No RPM found to validate')
    for path in paths:
        validate(path)
