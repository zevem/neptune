"""Small samples must retain the slowest observations in reported tail latency."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("measure_scale", Path(__file__).with_name("measure-scale.py"))
scale = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scale)


class ScaleMetricsTests(unittest.TestCase):
    def test_nearest_rank_percentiles_include_small_sample_tails(self):
        samples = [500, 20, 40, 10, 30]
        self.assertEqual(scale.percentile(samples, .5), 30)
        self.assertEqual(scale.percentile(samples, .95), 500)
        self.assertEqual(scale.percentile(samples, .99), 500)
        self.assertEqual(scale.percentile([7], .99), 7)
        self.assertIsNone(scale.percentile([], .99))

    def test_only_the_daemon_of_this_data_root_is_measured(self):
        with tempfile.TemporaryDirectory() as directory:
            proc = Path(directory) / "proc"
            data = Path(directory) / "data"
            def process(pid, *arguments):
                (proc / str(pid)).mkdir(parents=True)
                (proc / str(pid) / "cmdline").write_bytes(b"".join(a.encode() + b"\0" for a in arguments))
            process(10, "/bin/neptune", "--session-daemon", str(data / "sessions"))
            process(11, "/bin/neptune", "--session-daemon", str(data / "sessions-other"))
            process(12, "/bin/neptune", "--data-root", str(data))
            process(13, "/bin/sh", "-c", "--session-daemon " + str(data / "sessions"))
            (proc / "self").mkdir()
            self.assertEqual(scale.session_daemons(data, proc), [10])

    def test_neptune_and_its_daemon_are_compared_as_one(self):
        ticks = scale.os.sysconf("SC_CLK_TCK")
        before = {"cpu_ticks": 0, "context_switches": 100, "rss_bytes": 10, "threads": 4, "start_ticks": 7}
        after = {"cpu_ticks": ticks, "context_switches": 300, "rss_bytes": 30, "threads": 5, "start_ticks": 7}
        used = scale.usage(before, after, 2)
        self.assertEqual(used["cpu_core_percent"], 50)
        self.assertEqual(used["context_switches_per_second"], 100)
        self.assertEqual(used["rss_delta_bytes"], 20)
        both = scale.combined([used, used])
        self.assertEqual((both["cpu_core_percent"], both["rss_bytes"], both["threads"]), (100, 60, 10))
        with self.assertRaises(RuntimeError):
            scale.usage(before, {**after, "start_ticks": 8}, 2)


if __name__ == "__main__":
    unittest.main()
