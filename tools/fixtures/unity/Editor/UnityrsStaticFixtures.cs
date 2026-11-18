#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;
using Object = UnityEngine.Object;

static partial class UnityrsFixtureBuilder
{
    static void BuildStatic(Build build, RunReport report, Scene scene, string stamp)
    {
        string dir = null;
        if (!report.Try("Prepare static folder", StaticCases, () => dir = Folder(Assets + "/Static"))) return;
        if (report.Try("Prepare " + BatchBundle, new[] { BatchBundle },
            () => build.AddBundle(BatchBundle, BuildBatchScene(dir, scene, stamp))))
            build.StaticBundles.Add(BatchBundle, BatchBundle);

        const string bundle = "static-objects";
        var assets = new List<string>();
        void Add(string name, Func<string[]> buildAssets)
        {
            if (report.Try("Prepare " + name, new[] { name }, () => assets.AddRange(buildAssets())))
                build.StaticBundles.Add(name, bundle);
        }
        Add("static-primitives", () => new[] { BuildPrimitives(dir) });
        Add("static-materials", () => new[] { BuildMaterials(dir), dir + "/metallic.mat", dir + "/unlit.mat", dir + "/checker.asset" });
        Add("static-lod", () => new[] { BuildLod(dir) });
        Add("static-skin", () => {
            string mesh = dir + "/four-influence.asset";
            return new[] { BuildSkinPrefab(dir, Save(NewSkin(), mesh)), mesh };
        });
        if (assets.Count > 0) build.AddBundle(bundle, assets.ToArray());
    }

    static string BuildPrimitives(string dir)
    {
        var root = new GameObject("Primitives");
        try
        {
            var types = new[] { PrimitiveType.Cube, PrimitiveType.Cylinder, PrimitiveType.Sphere,
                PrimitiveType.Capsule, PrimitiveType.Plane, PrimitiveType.Quad };
            for (int i = 0; i < types.Length; ++i)
            {
                var primitive = Primitive(root.transform, types[i], types[i].ToString(), new Vector3(i * 3, 0, 0));
                Require(primitive.sharedMaterial != null, "Expected the Built-in default material.");
            }
            return SavePrefab(root, dir + "/primitives.prefab");
        }
        finally { Object.DestroyImmediate(root); }
    }

    static MeshRenderer Primitive(Transform parent, PrimitiveType type, string name, Vector3 position)
    {
        var go = GameObject.CreatePrimitive(type);
        go.name = name;
        go.transform.SetParent(parent, false);
        go.transform.localPosition = position;
        return go.GetComponent<MeshRenderer>();
    }

    static string BuildMaterials(string dir)
    {
        var tex = new Texture2D(2, 2, TextureFormat.RGBA32, false) { name = "Checker" };
        tex.SetPixels32(new[] {
            new Color32(20, 100, 180, 255), new Color32(230, 180, 40, 255),
            new Color32(80, 20, 120, 128), new Color32(200, 240, 160, 0)
        });
        tex.Apply();
        tex = Save(tex, dir + "/checker.asset");

        var standard = Shader.Find("Standard");
        var unlit = Shader.Find("Unlit/Texture");
        Require(standard != null && unlit != null, "The Built-in Standard and Unlit/Texture shaders are required.");
        var metallic = new Material(standard) { name = "Metallic", color = new Color(.4f, .6f, .8f, 1) };
        metallic.SetFloat("_Metallic", .625f);
        metallic.SetFloat("_Glossiness", .375f);
        metallic.SetColor("_EmissionColor", new Color(.125f, .25f, .5f, 1));
        metallic.EnableKeyword("_EMISSION");
        metallic.mainTexture = tex;
        metallic.mainTextureScale = new Vector2(2, 3);
        metallic.mainTextureOffset = new Vector2(.25f, .5f);
        metallic = Save(metallic, dir + "/metallic.mat");
        var flat = Save(new Material(unlit) { name = "Unlit", mainTexture = tex }, dir + "/unlit.mat");

        var root = new GameObject("BuiltInMaterials");
        try
        {
            Primitive(root.transform, PrimitiveType.Cube, "Metallic", Vector3.left).sharedMaterial = metallic;
            Primitive(root.transform, PrimitiveType.Quad, "Unlit", Vector3.right).sharedMaterial = flat;
            return SavePrefab(root, dir + "/materials.prefab");
        }
        finally { Object.DestroyImmediate(root); }
    }

    static string BuildLod(string dir)
    {
        var root = new GameObject("LodGroup");
        try
        {
            var near = Primitive(root.transform, PrimitiveType.Sphere, "Near", Vector3.zero);
            var far = Primitive(root.transform, PrimitiveType.Cube, "Far", Vector3.zero);
            var group = root.AddComponent<LODGroup>();
            group.SetLODs(new[] { new LOD(.6f, new Renderer[] { near }), new LOD(.2f, new Renderer[] { far }) });
            group.RecalculateBounds();
            return SavePrefab(root, dir + "/lod.prefab");
        }
        finally { Object.DestroyImmediate(root); }
    }

    static string BuildBatchScene(string dir, Scene scene, string stamp)
    {
        // Use readable original meshes. The default primitive references are
        // exercised in the other bundle without depending on their readability.
        var mesh = Save(NewPanel("BatchPanel"), dir + "/batch-panel.asset");
        var source = Shader.Find("Standard");
        Require(source != null, "The Built-in Standard shader is required for static batching.");
        var material = Save(new Material(source) { name = "BatchMaterial" }, dir + "/batch.mat");
        GameObject first = null, second = null, runMarker = null;
        try
        {
            // The roots deliberately have different names and authored order.
            first = new GameObject("Zulu");
            second = new GameObject("Alpha");
            first.transform.position = new Vector3(3, 1, -2);
            second.transform.position = new Vector3(-1, 2, 4);
            second.transform.rotation = Quaternion.Euler(0, 45, 0);
            second.transform.localScale = new Vector3(2, 1, .5f);
            var objects = new[] { first, second };
            foreach (var obj in objects)
            {
                obj.AddComponent<MeshFilter>().sharedMesh = mesh;
                obj.AddComponent<MeshRenderer>().sharedMaterial = material;
            }

            // Confirm on the first Editor run that Combine creates a shared
            // mesh and nonzero serialized submesh ranges in the scene bundle.
            StaticBatchingUtility.Combine(objects, null);
            var combined = first.GetComponent<MeshFilter>().sharedMesh;
            Require(combined != null && combined != mesh && second.GetComponent<MeshFilter>().sharedMesh == combined
                && first.GetComponent<MeshRenderer>().isPartOfStaticBatch
                && second.GetComponent<MeshRenderer>().isPartOfStaticBatch,
                "StaticBatchingUtility.Combine did not create a shared static batch.");
            Require(combined.isReadable && combined.vertexCount > 0 && combined.vertexCount == mesh.vertexCount * objects.Length,
                "The shared static batch must contain the expected readable vertices.");
            if (!AssetDatabase.Contains(combined))
            {
                combined.name = "CombinedBatchPanels";
                combined.hideFlags = HideFlags.None;
                var saved = Save(combined, dir + "/batch-combined.asset");
                foreach (var obj in objects)
                {
                    var filter = obj.GetComponent<MeshFilter>();
                    if (filter.sharedMesh != saved) filter.sharedMesh = saved;
                }
            }

            string path = dir + "/batch.unity";
            runMarker = new GameObject(stamp);
            Require(EditorSceneManager.SaveScene(scene, path, true), "Could not save " + path);
            return path;
        }
        finally
        {
            if (first != null) Object.DestroyImmediate(first);
            if (second != null) Object.DestroyImmediate(second);
            if (runMarker != null) Object.DestroyImmediate(runMarker);
        }
    }

    static Mesh NewPanel(string name)
    {
        var mesh = new Mesh { name = name };
        mesh.vertices = new[] { new Vector3(-1, 0, 0), new Vector3(1, 0, 0),
            new Vector3(-1, 2, 0), new Vector3(1, 2, 0) };
        mesh.uv = new[] { Vector2.zero, Vector2.right, Vector2.up, Vector2.one };
        mesh.triangles = new[] { 0, 2, 1, 1, 2, 3 };
        mesh.RecalculateNormals();
        mesh.RecalculateBounds();
        return mesh;
    }

    static Mesh NewSkin()
    {
        var mesh = NewPanel("FourInfluencePanel");
        mesh.bindposes = new[] { Matrix4x4.identity, Matrix4x4.Translate(Vector3.down),
            Matrix4x4.Translate(Vector3.left), Matrix4x4.Translate(Vector3.right) };
        mesh.boneWeights = new[] { Weight(0, 1, 2, 3), Weight(1, 0, 3, 2), Weight(2, 3, 0, 1), Weight(3, 2, 1, 0) };
        var zero = new Vector3[4];
        mesh.AddBlendShapeFrame("Lift", 25, new[] { Vector3.zero, Vector3.zero,
            Vector3.forward * .125f, Vector3.forward * .25f }, zero, zero);
        mesh.AddBlendShapeFrame("Lift", 80, new[] { Vector3.zero, Vector3.zero,
            Vector3.forward * .5f, Vector3.forward }, zero, zero);
        return mesh;
    }

    static BoneWeight Weight(int first, int second, int third, int fourth) => new BoneWeight {
        boneIndex0 = first, boneIndex1 = second, boneIndex2 = third, boneIndex3 = fourth,
        weight0 = .5f, weight1 = .25f, weight2 = .1875f, weight3 = .0625f
    };

    static string BuildSkinPrefab(string dir, Mesh mesh)
    {
        var root = new GameObject("SkinnedPanel");
        try
        {
            AddSkins(root.transform, mesh);
            root.transform.localPosition = new Vector3(2, 1, -3);
            root.transform.localRotation = Quaternion.Euler(20, 30, 40);
            root.transform.localScale = new Vector3(1, 2, .5f);
            return SavePrefab(root, dir + "/skin.prefab");
        }
        finally { Object.DestroyImmediate(root); }
    }

    static void AddSkins(Transform parent, Mesh mesh)
    {
        var geometry = Child(parent, "Skin", Vector3.zero);
        var first = Child(geometry, "RootBone", Vector3.zero);
        var offsets = new[] { Vector3.up, Vector3.right, Vector3.left };
        var bones = new[] { first, Child(first, "Upper", offsets[0]),
            Child(first, "Right", offsets[1]), Child(first, "Left", offsets[2]) };
        var primitive = GameObject.CreatePrimitive(PrimitiveType.Cube);
        var defaultMaterial = primitive.GetComponent<MeshRenderer>().sharedMaterial;
        Object.DestroyImmediate(primitive);
        Require(defaultMaterial != null, "Could not load the Built-in default material for the skin.");
        // Both renderers use the same bind transforms, but carry separate weights.
        for (int i = 0; i < 2; ++i)
        {
            var obj = Child(geometry, i == 0 ? "RendererA" : "RendererB", Vector3.zero);
            var renderer = obj.gameObject.AddComponent<SkinnedMeshRenderer>();
            renderer.sharedMesh = mesh;
            renderer.sharedMaterial = defaultMaterial;
            renderer.rootBone = first;
            renderer.bones = bones;
            renderer.quality = SkinQuality.Bone4;
            renderer.updateWhenOffscreen = true;
            renderer.SetBlendShapeWeight(0, i == 0 ? 12.5f : 50);
        }
    }
}
#endif
