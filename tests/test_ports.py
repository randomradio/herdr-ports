#!/usr/bin/env python3
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import herdr_ports as ports  # noqa: E402


SS_SAMPLE = """
LISTEN 0 4096 127.0.0.1:4242 0.0.0.0:* users:(("node",pid=2211,fd=23))
LISTEN 0 4096 127.0.0.1:5242 0.0.0.0:* users:(("python",pid=4410,fd=4))
LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:(("sshd",pid=1,fd=3))
"""

LSOF_SAMPLE = """
COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
node     2211 you   23u  IPv4  0xabc      0t0  TCP 127.0.0.1:4242 (LISTEN)
python   4410 you    4u  IPv4  0xdef      0t0  TCP 127.0.0.1:5242 (LISTEN)
sshd        1 root   3u  IPv4  0x111      0t0  TCP *:22 (LISTEN)
"""


class ParseTests(unittest.TestCase):
    def test_parse_ss_skips_sshd_and_keeps_dev_ports(self) -> None:
        listeners = ports.parse_listeners(SS_SAMPLE)
        ports_found = [item.port for item in listeners]
        self.assertEqual(ports_found, [4242, 5242])
        self.assertEqual(listeners[0].process, "node")
        self.assertEqual(listeners[0].pid, 2211)
        self.assertEqual(listeners[1].process, "python")

    def test_parse_lsof(self) -> None:
        listeners = ports.parse_listeners(LSOF_SAMPLE)
        self.assertEqual([item.port for item in listeners], [4242, 5242])
        self.assertEqual(listeners[0].addr, "127.0.0.1")
        self.assertEqual(listeners[0].pid, 2211)

    def test_localhost_urls(self) -> None:
        self.assertEqual(ports.parse_localhost_port("http://localhost:4242/app"), 4242)
        self.assertEqual(ports.parse_localhost_port("http://127.0.0.1:5242"), 5242)
        self.assertEqual(
            ports.parse_localhost_port("http://herdr.mo.localhost:8000"), 8000
        )
        self.assertIsNone(ports.parse_localhost_port("http://example.com:4242"))

    def test_workspace_url(self) -> None:
        self.assertEqual(ports.workspace_slug("spark: ~"), "spark")
        self.assertEqual(
            ports.workspace_url("mo", 8000),
            "http://herdr.mo.localhost:8000",
        )




    def test_bind_detects_occupied_port(self) -> None:
        sock = __import__("socket").socket()
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
        try:
            self.assertIsNotNone(ports.local_port_holder(port))
        finally:
            sock.close()


if __name__ == "__main__":
    unittest.main()
