#if UNITY_EDITOR
using System.Collections.Generic;
using System.Linq;
using UnityEngine;

static partial class UnityrsFixtureBuilder
{
    static Mesh NewHumanoidSkin(Transform root, Transform[] bones)
    {
        // One rigidly weighted box per joint, in the renderer's bind space.
        var vertices = new List<Vector3>();
        var weights = new List<BoneWeight>();
        var triangles = new List<int>();
        int[] boxTriangles = {
            0, 2, 1, 1, 2, 3, 4, 5, 6, 5, 7, 6,
            0, 1, 4, 1, 5, 4, 2, 6, 3, 3, 6, 7,
            0, 4, 2, 2, 4, 6, 1, 3, 5, 3, 7, 5
        };
        for (int i = 0; i < bones.Length; i++)
        {
            var bone = bones[i];
            Vector3 start = root.InverseTransformPoint(bone.position);
            Vector3 direction = bone.childCount == 0 ? Vector3.zero
                : root.InverseTransformPoint(bone.GetChild(0).position) - start;
            float length = direction.magnitude;
            var axes = length == 0 ? Quaternion.identity : Quaternion.FromToRotation(Vector3.up, direction / length);
            float halfWidth = Mathf.Clamp(length * .28f, .04f, .105f);
            float halfHeight = Mathf.Max(.06f, length * .5f);
            int first = vertices.Count;
            for (int corner = 0; corner < 8; corner++)
            {
                var offset = new Vector3(
                    (corner & 1) == 0 ? -halfWidth : halfWidth,
                    (corner & 2) == 0 ? -halfHeight : halfHeight,
                    (corner & 4) == 0 ? -halfWidth : halfWidth);
                vertices.Add(start + direction * .5f + axes * offset);
                weights.Add(new BoneWeight { boneIndex0 = i, weight0 = 1 });
            }
            foreach (int index in boxTriangles) triangles.Add(first + index);
        }

        var mesh = new Mesh { name = "HumanoidBoxes" };
        mesh.SetVertices(vertices);
        mesh.SetTriangles(triangles, 0);
        mesh.boneWeights = weights.ToArray();
        mesh.bindposes = bones.Select(bone => bone.worldToLocalMatrix * root.localToWorldMatrix).ToArray();
        mesh.RecalculateNormals();
        mesh.RecalculateBounds();
        return mesh;
    }
}
#endif
