#if UNITY_EDITOR
using System.Linq;
using UnityEditor;
using UnityEngine;
using UnityEngine.Rendering;
using Object = UnityEngine.Object;

static partial class UnityrsFixtureBuilder
{
    // tools/parity/anim_fixture_variants.py checks these names and values in the built bundle.
    const string UrpLit = "Universal Render Pipeline/Lit";
    const string UrpUnlit = "Universal Render Pipeline/Unlit";

    static string[] BuildUrpMaterials(string dir)
    {
        var baseMap = Save(NewTexture("UrpBase", 4, 4), dir + "/base.asset");
        var normalMap = Save(NewTexture("UrpNormal", 2, 2), dir + "/normal.asset");
        var litShader = Shader.Find(UrpLit);
        var unlitShader = Shader.Find(UrpUnlit);
        Require(litShader != null && unlitShader != null, "The URP " + UrpLit + " and " + UrpUnlit + " shaders are required.");

        var lit = new Material(litShader) { name = "UrpLit" };
        Properties(lit, "_BaseMap", "_BaseColor", "_BumpMap", "_BumpScale", "_Metallic", "_Smoothness", "_EmissionColor");
        lit.SetTexture("_BaseMap", baseMap);
        lit.SetTextureScale("_BaseMap", new Vector2(2, 3));
        lit.SetTextureOffset("_BaseMap", new Vector2(.25f, .5f));
        lit.SetColor("_BaseColor", new Color(.25f, .5f, .75f, 1));
        lit.SetTexture("_BumpMap", normalMap);
        lit.SetFloat("_BumpScale", 1.5f);
        lit.SetFloat("_Metallic", .625f);
        lit.SetFloat("_Smoothness", .375f);
        // URP's material validation derives these keywords from the same
        // properties, so a reimport keeps them: a normal map and a nonblack
        // emission colour with realtime emissive GI.
        lit.SetColor("_EmissionColor", new Color(.125f, .25f, .5f, 1));
        lit.globalIlluminationFlags = MaterialGlobalIlluminationFlags.RealtimeEmissive;
        lit.EnableKeyword("_NORMALMAP");
        lit.EnableKeyword("_EMISSION");
        string litPath = dir + "/lit.mat";
        Save(lit, litPath);

        var unlit = new Material(unlitShader) { name = "UrpUnlit" };
        Properties(unlit, "_BaseMap", "_BaseColor", "_AlphaClip", "_Cutoff");
        unlit.SetTexture("_BaseMap", baseMap);
        unlit.SetColor("_BaseColor", new Color(1, .5f, .25f, 1));
        unlit.SetFloat("_AlphaClip", 1);
        unlit.SetFloat("_Cutoff", .375f);
        unlit.EnableKeyword("_ALPHATEST_ON");
        unlit.renderQueue = (int)RenderQueue.AlphaTest;
        string unlitPath = dir + "/unlit.mat";
        Save(unlit, unlitPath);

        // URP's material postprocessor may revalidate the saved materials.
        // Read them back so a dropped keyword fails here, not in the repository.
        AssetDatabase.SaveAssets();
        lit = Keywords(litPath, "_NORMALMAP", "_EMISSION");
        unlit = Keywords(unlitPath, "_ALPHATEST_ON");

        var root = new GameObject("UrpMaterials");
        try
        {
            Primitive(root.transform, PrimitiveType.Cube, "Lit", Vector3.left).sharedMaterial = lit;
            Primitive(root.transform, PrimitiveType.Quad, "Unlit", Vector3.right).sharedMaterial = unlit;
            string prefab = SavePrefab(root, dir + "/materials.prefab");
            return new[] { prefab, litPath, unlitPath, dir + "/base.asset", dir + "/normal.asset" };
        }
        finally { Object.DestroyImmediate(root); }
    }

    // Alpha 64 (.25) on every third texel falls below the Unlit _Cutoff .375,
    // so the alpha-tested material rejects some texels and keeps the rest.
    static Texture2D NewTexture(string name, int width, int height)
    {
        var tex = new Texture2D(width, height, TextureFormat.RGBA32, false) { name = name };
        var pixels = new Color32[width * height];
        for (int i = 0; i < pixels.Length; ++i)
            pixels[i] = new Color32((byte)(40 * i), (byte)(255 - 30 * i), (byte)(90 + 10 * i), (byte)(i % 3 == 0 ? 64 : 255));
        tex.SetPixels32(pixels);
        tex.Apply();
        return tex;
    }

    static void Properties(Material material, params string[] names)
    {
        var missing = names.Where(name => !material.HasProperty(name)).ToArray();
        Require(missing.Length == 0, material.shader.name + " has no " + string.Join(", ", missing) + " property.");
    }

    static Material Keywords(string path, params string[] keywords)
    {
        var material = AssetDatabase.LoadAssetAtPath<Material>(path);
        Require(material != null, "Could not reload " + path);
        var missing = keywords.Where(keyword => !material.IsKeywordEnabled(keyword)).ToArray();
        Require(missing.Length == 0, "URP dropped the " + string.Join(", ", missing) + " keyword from " + path + ".");
        return material;
    }
}
#endif
