#if UNITY_EDITOR
using UnityEngine.Rendering;
#if UNITYRS_URP
using UnityEngine.Rendering.Universal;
#endif

namespace UnityrsFixtures
{
    // Its own assembly (Unityrs.Fixtures.Urp.asmdef) references the URP
    // runtime, and the asmdef defines UNITYRS_URP only when the
    // com.unity.render-pipelines.universal package is installed. A Built-in
    // project compiles the fallback branch, without any URP type.
    public static class UnityrsUrpPipeline
    {
#if UNITYRS_URP
        public static bool PackageInstalled => true;
        public static bool Active => GraphicsSettings.currentRenderPipeline is UniversalRenderPipelineAsset;
#else
        public static bool PackageInstalled => false;
        public static bool Active => false;
#endif

        public static string Current => GraphicsSettings.currentRenderPipeline == null
            ? "Built-in" : GraphicsSettings.currentRenderPipeline.GetType().Name;
    }
}
#endif
