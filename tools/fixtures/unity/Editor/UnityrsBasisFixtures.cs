#if UNITY_EDITOR
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

// Optional coverage: the root basis matrix. The compact rig's muscle clip on a
// neutral Avatar, once per combination of the three keep-original flags
// (orientation, Y, XZ), every clip in one bundle and played back on one prefab,
// so their root and hips samples differ only by the basis. AnimationClipSettings
// has no start-at-origin field; the built clips store it on whatever the XZ flag.
// Every combination but the all-off one is a root basis refusal probe; the
// all-off one's residual alone says whether it needs a hips correction.
static partial class UnityrsFixtureBuilder
{
    const string BasisBundle = "humanoid-root-basis";
    // Bit 2 is the orientation flag, bit 1 Y, bit 0 XZ.
    static readonly string[] BasisCases = Enumerable.Range(0, 8).Select(BasisCase).ToArray();

    static string BasisCase(int flags) =>
        "humanoid-root-basis-o" + (flags >> 2 & 1) + "-y" + (flags >> 1 & 1) + "-xz" + (flags & 1);

    static bool[] BasisFlags(int flags) => new[] { (flags & 4) != 0, (flags & 2) != 0, (flags & 1) != 0 };

    internal sealed partial class ReplayCase
    {
        // The authored keep-original flags (orientation, Y, XZ) of a basis matrix clip.
        public bool[] RootBasis;
    }

    static void BuildBasisMatrix(Build build)
    {
        var rig = new GameObject("HumanoidRig");
        GameObject root = null;
        Avatar avatar = null;
        try
        {
            var hips = BuildHumanoidSkeleton(rig.transform, false);
            var skeleton = rig.GetComponentsInChildren<Transform>();
            avatar = AvatarBuilder.BuildHumanAvatar(rig, DescribeHumanoid(skeleton, false));
            Require(avatar != null && avatar.isValid && avatar.isHuman,
                "AvatarBuilder.BuildHumanAvatar returned an invalid Humanoid Avatar.");
            avatar.name = BasisBundle;
            avatar.hideFlags = HideFlags.None;

            string dir = Folder(Assets + "/" + BasisBundle);
            var animator = SaveCoverageRig(rig, hips, skeleton, avatar, dir, out var assets);
            var clips = new AnimationClip[BasisCases.Length];
            for (int i = 0; i < clips.Length; i++)
            {
                clips[i] = AuthorHumanoidClip(dir, BasisCases[i], bodyMotion: false);
                SetNeutralSettings(clips[i], 0, BasisFlags(i));
            }
            animator.runtimeAnimatorController = Controller(assets[assets.Length - 1], clips);

            root = new GameObject("HumanoidFixture");
            rig.transform.SetParent(root.transform, false);
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            var clipPaths = clips.Select(AssetDatabase.GetAssetPath).ToArray();
            build.AddCoverageBundle(BasisBundle, assets.Concat(clipPaths).Concat(new[] { prefab }).ToArray());
            for (int i = 0; i < clips.Length; i++)
            {
                var replay = HumanoidReplay(BasisBundle, prefab, assets[0], clipPaths[i], BasisCases[i]);
                replay.Optional = true;
                replay.RootBasis = BasisFlags(i);
                replay.ExpectedRefusal = i == 0 ? "" : "unsupported_humanoid_root_basis";
                build.Replays.Add(replay);
            }
        }
        finally
        {
            Object.DestroyImmediate(root == null ? rig : root);
            if (avatar != null && !AssetDatabase.Contains(avatar)) Object.DestroyImmediate(avatar);
        }
    }
}
#endif
