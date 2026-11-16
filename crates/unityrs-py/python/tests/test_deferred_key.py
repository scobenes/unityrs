"""Deferred key staging belongs to the active input, including failed loads."""
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
from binding_y5_round2 import encrypted_input
import unityrs
from unityrs.helpers import ArchiveStorageManager as settings


class DeferredKey(unittest.TestCase):
    def test_key_snapshot_shares_the_active_input_memory_limit(self):
        inputs, _, raw, _ = encrypted_input()
        previous = settings.DECRYPT_KEY
        try:
            key = bytearray.fromhex(inputs['key'])
            unityrs.set_assetbundle_decrypt_key(key)
            # Caller mutation after setter validation is allowed. This staging
            # alone fits exactly, but not beside the retained bundle input.
            key.extend(bytes(100_000 - 256 - len(key)))
            env = unityrs.Environment(limits=unityrs.Limits(max_input_memory=100_000))
            with self.assertRaises(unityrs.LimitError):
                env.load_file(raw, name='synthetic.bundle')
        finally:
            settings.DECRYPT_KEY = previous

    def test_mutable_key_work_does_not_accumulate_across_inputs(self):
        inputs, _, raw, header_end = encrypted_input()
        previous = settings.DECRYPT_KEY
        try:
            unityrs.set_assetbundle_decrypt_key(bytearray.fromhex(inputs['key']))
            env = unityrs.Environment(limits=unityrs.Limits(max_decode_work=1000))
            # Each input ends after the complete encryption envelope. Reading
            # the absent block info fails; its key copy belongs to that input,
            # so a later attempt must still reach the same error.
            for attempt in range(70):
                with self.subTest(attempt=attempt), self.assertRaises(struct.error):
                    env.load_file(raw[:header_end + 70], name='truncated.bundle')
        finally:
            settings.DECRYPT_KEY = previous
