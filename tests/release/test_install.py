"""Tests for the managed install handoff in scripts/install.sh."""
import os
from pathlib import Path
import pty
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ManagedInstallHandoff(unittest.TestCase):
    def test_pipe_install_forwards_selection_and_uses_the_terminal(self):
        for selection, expected in [({'HOSHI_VERSION': 'v0.2.0'}, ['--channel', 'stable', '--version', 'v0.2.0']),
                                    ({'HOSHI_CHANNEL': 'nightly'}, ['--channel', 'nightly'])]:
            with self.subTest(selection=selection), tempfile.TemporaryDirectory(prefix=f'hoshi-install-test-{os.getpid()}-') as temporary:
                directory = Path(temporary)
                fake = directory / 'hoshi'
                fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\n')
                fake.chmod(0o755)
                capture = directory / 'arguments'
                env = {key: value for key, value in os.environ.items() if key not in {'HOSHI_VERSION', 'HOSHI_CHANNEL'}}
                env.update(selection)
                env.update(PATH=str(directory) + os.pathsep + env['PATH'], HOSHI_MANAGED_DIR=str(directory / 'managed'), CAPTURE=str(capture))
                pid, terminal = pty.fork()
                if pid == 0:
                    os.execve('/bin/sh', ['/bin/sh', str(ROOT / 'scripts/install.sh')], env)
                try:
                    while True:
                        try:
                            if not os.read(terminal, 4096):
                                break
                        except OSError:
                            break
                finally:
                    os.close(terminal)
                _, status = os.waitpid(pid, 0)
                self.assertEqual(os.waitstatus_to_exitcode(status), 0)
                self.assertEqual(capture.read_text().splitlines(), ['update', '--install-dir', str(directory / 'managed'), *expected])


if __name__ == '__main__':
    unittest.main()
