# Ported from UnityPy 1.25.2 (MIT): export/AudioClipConverter.py:13-49. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Audio export through the current-value native class adapter."""
def extract_audioclip_samples(audio, convert_pcm_float=True):
    from ..classes._exports import _audio_samples
    if not convert_pcm_float:
        raise NotImplementedError("unconverted floating-point audio export is not exposed")
    return _audio_samples(audio)
