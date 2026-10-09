"""Checks for the Linux retained-memory probe's measurement adapter."""
import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "measure_memory", Path(__file__).with_name("measure-memory.py")
)
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


class MemoryTests(unittest.TestCase):
    def test_resident_proportional_and_private_memory_have_distinct_scopes(self):
        status = "VmRSS: 1000 kB\nVmHWM: 1200 kB\nThreads: 7\n"
        rollup = (
            "Rss: 1000 kB\nPss: 600 kB\nShared_Clean: 500 kB\n"
            "Private_Clean: 100 kB\nPrivate_Dirty: 300 kB\n"
        )
        with patch.object(Path, "read_text", side_effect=[status, rollup]):
            self.assertEqual(probe.memory(123), {
                "rss_bytes": 1000 * 1024,
                "pss_bytes": 600 * 1024,
                "private_bytes": 400 * 1024,
                "peak_rss_bytes": 1200 * 1024,
            })

    def test_missing_process_is_a_failed_sample(self):
        with patch.object(Path, "read_text", side_effect=FileNotFoundError):
            with self.assertRaises(FileNotFoundError):
                probe.memory(123)

    def test_missing_accounting_field_is_not_silently_reported_as_zero(self):
        with patch.object(Path, "read_text", return_value="VmRSS: 1000 kB\n"):
            with self.assertRaises(KeyError):
                probe.memory(123)


if __name__ == "__main__":
    unittest.main()
