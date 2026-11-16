"""Installed wheel parity for the standalone decryptor and shader stages."""
import unittest
import UnityPy
import unityrs
import binding_y6b_crypto_shader as observations


class CryptoShader(unittest.TestCase):
    def test_block_indices_wrap_unsigned_64_bits(self):
        self.compare('crypto_block_indices')

    def compare(self, name):
        call = getattr(observations, 'observe_' + name)
        self.assertEqual(UnityPy.__version__, '1.25.2')
        self.assertEqual(call(unityrs), call(UnityPy))

    def test_synthetic_aes_vectors_and_lengths(self):
        self.compare('crypto_keys')

    def test_synthetic_decryptor_fields_blocks_and_helpers(self):
        self.compare('crypto_decryptor')

    def test_synthetic_key_search_and_verbose_output(self):
        self.compare('crypto_bruteforce')

    def test_shader_converter_stages(self):
        self.compare('shader_stages')

    def test_shader_programs_current_fields_and_tier_order(self):
        self.compare('shader_programs')

    def test_shader_platform_and_program_type_matrix(self):
        self.compare('shader_platforms')

    def test_shader_stages_export_selected_mutable_programs(self):
        self.compare('shader_mutable_stages')

    def test_shader_stages_leave_unused_programs_untouched(self):
        self.compare('shader_unused_programs')


if __name__ == '__main__':
    unittest.main()
