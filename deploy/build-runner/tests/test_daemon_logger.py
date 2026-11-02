from pathlib import Path
import os
import subprocess
import tempfile
import unittest

class LoggerTests(unittest.TestCase):
    def test_actual_logger_drains_pipe_after_storage_cap_without_breaking_writer(self):
        helper = Path(__file__).resolve().parents[1] / "daemon-logger.sh"
        payload = b"0123456789abcdef" * (128 * 1024)
        with tempfile.TemporaryFile() as logfile:
            process = subprocess.Popen(["/bin/sh", str(helper)], stdin=subprocess.PIPE, stdout=logfile, stderr=subprocess.PIPE, bufsize=0)
            try:
                offset = 0
                while offset < len(payload):
                    offset += os.write(process.stdin.fileno(), payload[offset:])
                process.stdin.close()
                self.assertEqual(process.wait(timeout=3), 0)
                logfile.seek(0)
                self.assertEqual(logfile.read(), payload[:24000])
            finally:
                if not process.stdin.closed:
                    process.stdin.close()
                if process.poll() is None:
                    process.kill()
                    process.wait()
                process.stderr.close()

if __name__ == "__main__":
    unittest.main()
