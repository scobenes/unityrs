"""Initial shared-reader cursor and concurrent lazy publication regressions."""
import sys
import threading
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
from binding_y5_objects import _asset, _container_asset
import unityrs


class ReaderPublication(unittest.TestCase):
    def test_initial_cursor_preserves_metadata_parser_position(self):
        data = _asset()
        obj = unityrs.Environment(data).objects[0]
        self.assertEqual(obj.assets_file.reader.Position, 277)
        self.assertEqual(obj.Position, 277)
        self.assertEqual(obj.read_bytes(4), data[277:281])
        self.assertEqual(obj.assets_file.reader.Position, 281)
        obj.reset()
        self.assertEqual(obj.read_int(), 7)

    def test_initial_cursor_follows_eager_assetbundle_read(self):
        data = _container_asset('same', 1)
        file = unityrs.Environment(data).objects[0].assets_file
        self.assertEqual(file.reader.Position, 1180)
        self.assertEqual(file.objects[9].Position, 1180)
        self.assertEqual(file.reader.read(4), data[1180:1184])

    def test_concurrent_first_access_publishes_one_shared_reader(self):
        env = unityrs.Environment(_asset())
        file = env.objects[0].assets_file
        native, lock = file._native, env._lock
        first_inside = threading.Event()
        second_attempted = threading.Event()
        values, errors = [], []

        class PublicationLock:
            def __enter__(self):
                if threading.current_thread().name == 'reader-second':
                    second_attempted.set()
                lock.acquire()

            def __exit__(self, *args):
                lock.release()

        class PausedNative:
            def __getattr__(self, name):
                return getattr(native, name)

            def serialized_reader_info(self):
                # Pause at the native boundary where the GIL may be released.
                # The second caller either reaches this boundary too (the
                # bug), or waits on the environment lock (the fixed path).
                if threading.current_thread().name == 'reader-first':
                    first_inside.set()
                    if not second_attempted.wait(5):
                        raise AssertionError('second reader access did not start')
                else:
                    second_attempted.set()
                return native.serialized_reader_info()

        def read():
            try:
                values.append(file.reader)
            except BaseException as error:
                errors.append(error)

        env._lock, file._native = PublicationLock(), PausedNative()
        first = threading.Thread(target=read, name='reader-first', daemon=True)
        second = threading.Thread(target=read, name='reader-second', daemon=True)
        try:
            first.start()
            self.assertTrue(first_inside.wait(5), 'first reader did not reach native boundary')
            second.start()
            first.join(5)
            second.join(5)
            self.assertFalse(first.is_alive() or second.is_alive(), 'reader access deadlocked')
            self.assertEqual(errors, [])
            self.assertEqual(len(values), 2)
            self.assertIs(values[0], values[1])
            values[0].Position = 11
            self.assertEqual(values[1].Position, 11)
            self.assertIs(file.objects[1].reader, values[0])
        finally:
            second_attempted.set()
            env._lock, file._native = lock, native


if __name__ == '__main__':
    unittest.main()
