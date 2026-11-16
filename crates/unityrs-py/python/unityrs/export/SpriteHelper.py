# Ported from UnityPy 1.25.2 (MIT): export/SpriteHelper.py:44-127. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
def get_image_from_sprite(m_Sprite):
    from ..classes._images_sprite import get_sprite_image
    return get_sprite_image(m_Sprite)


__all__ = ["get_image_from_sprite"]
