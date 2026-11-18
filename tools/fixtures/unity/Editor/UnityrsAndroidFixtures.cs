#if UNITY_EDITOR
using System.Collections.Generic;
using System.IO;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

static partial class UnityrsFixtureBuilder
{
    sealed class AndroidTexture
    {
        public string Name;
        public int Width, Height;
        public TextureImporterFormat Format;
        public bool Alpha;
    }

    // tools/parity/anim_fixture_variants.py expects each built format and size.
    // 40x24 leaves partial 6x6 ASTC blocks on both axes.
    static readonly AndroidTexture[] AndroidTextures = {
        new AndroidTexture { Name = "astc-4x4", Width = 64, Height = 64, Format = TextureImporterFormat.ASTC_4x4, Alpha = true },
        new AndroidTexture { Name = "astc-6x6", Width = 40, Height = 24, Format = TextureImporterFormat.ASTC_6x6, Alpha = true },
        new AndroidTexture { Name = "astc-8x8", Width = 64, Height = 64, Format = TextureImporterFormat.ASTC_8x8, Alpha = true },
        new AndroidTexture { Name = "etc-rgb4", Width = 64, Height = 64, Format = TextureImporterFormat.ETC_RGB4 },
        new AndroidTexture { Name = "etc2-rgb4", Width = 64, Height = 64, Format = TextureImporterFormat.ETC2_RGB4 },
        new AndroidTexture { Name = "etc2-rgb4-punchthrough", Width = 64, Height = 64,
            Format = TextureImporterFormat.ETC2_RGB4_PUNCHTHROUGH_ALPHA, Alpha = true },
        new AndroidTexture { Name = "etc2-rgba8", Width = 64, Height = 64, Format = TextureImporterFormat.ETC2_RGBA8, Alpha = true },
    };

    static string[] BuildAndroidTextures(string dir)
    {
        var paths = new List<string>();
        foreach (var spec in AndroidTextures)
        {
            string path = dir + "/" + spec.Name + ".png";
            var source = new Texture2D(spec.Width, spec.Height, TextureFormat.RGBA32, false);
            try
            {
                source.SetPixels32(Gradient(spec.Width, spec.Height));
                source.Apply();
                File.WriteAllBytes(Path.Combine(Application.dataPath, path.Substring("Assets/".Length)), source.EncodeToPNG());
            }
            finally { Object.DestroyImmediate(source); }
            AssetDatabase.ImportAsset(path, ImportAssetOptions.ForceSynchronousImport);
            var importer = AssetImporter.GetAtPath(path) as TextureImporter;
            Require(importer != null, "Could not import " + path + " as a texture.");
            importer.textureType = TextureImporterType.Default;
            importer.npotScale = TextureImporterNPOTScale.None;
            importer.mipmapEnabled = false;
            importer.isReadable = false;
            importer.alphaSource = spec.Alpha ? TextureImporterAlphaSource.FromInput : TextureImporterAlphaSource.None;
            importer.alphaIsTransparency = false;
            importer.textureCompression = TextureImporterCompression.Compressed;
            importer.SetPlatformTextureSettings(new TextureImporterPlatformSettings {
                name = "Android", overridden = true, format = spec.Format, maxTextureSize = 2048,
                compressionQuality = 50, textureCompression = TextureImporterCompression.Compressed
            });
            importer.SaveAndReimport();
            var stored = importer.GetPlatformTextureSettings("Android");
            Require(stored.overridden && stored.format == spec.Format,
                "Unity did not keep the Android " + spec.Format + " override on " + path + ".");
            paths.Add(path);
        }
        return paths.ToArray();
    }

    // Distinct colours per block and binary alpha, so every format keeps
    // visible structure and the punch-through alpha survives compression.
    static Color32[] Gradient(int width, int height)
    {
        var pixels = new Color32[width * height];
        for (int y = 0; y < height; ++y)
            for (int x = 0; x < width; ++x)
                pixels[y * width + x] = new Color32((byte)(x * 255 / (width - 1)), (byte)(y * 255 / (height - 1)),
                    (byte)((x ^ y) * 4), (byte)(((x / 8 + y / 8) & 1) == 0 ? 255 : 0));
        return pixels;
    }
}
#endif
