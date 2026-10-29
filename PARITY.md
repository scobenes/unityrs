# UnityPy parity inventory

**Status:** P0, 2026-10-28. **Oracle:** UnityPy 1.25.2 with UnityPyBoost, in the oracle image (`DESIGN.md` section 3).

This is the inventory `DESIGN.md` section 3 asks for. It maps every public UnityPy API and behaviour to a tier, a verification level and a test, and gives a per-`TextureFormat` matrix. "Full parity" means every row here is green. It was built by reading UnityPy 1.25.2 as installed in the oracle image, and the claims about what UnityPy does with a texture format come from running it there (section 17).

`tools/parity/inventory.py check` fails when a UnityPy module, class, module-level function, module-level table or setting, class member, attrs field (the generated classes' included), package re-export, UnityPyBoost export or `TextureFormat` is missing from this file, or is named only under a wrong parent (section 21).

## 0. How to read the tables

**Citations.** `file.py:N` is a line of UnityPy 1.25.2, relative to the `UnityPy/` package. `HEAD file.py:N` is UnityPy git `417d595`, the HEAD oracle. Where a table says "lines are in X", a bare `:N` is a line of X.

**Tier** (`DESIGN.md` section 3):

| tier | meaning |
|---|---|
| A | bit-exact with UnityPy wherever UnityPy is right |
| B | UnityPy raises; unityrs produces correct output, checked against an independent reference |
| C | UnityPy is wrong; unityrs produces correct output, with a pinned divergence test (section 19) |
| Tol | compared within a stated metric and threshold (Sprite affine path, audio against FMOD) |
| HEAD | equal to UnityPy git `417d595` on Unity 6000.4/6000.5 fixtures (section 20) |
| U | unsupported until a sample and a reference exist: unityrs refuses the input with a named `Unsupported` error. Where UnityPy does not raise (the ETC 3DS formats return pixels nobody has checked), that is a documented divergence, not parity |
| gap | a deliberate `DESIGN.md` gap: UnityPy produces output (through FMOD) and unityrs reports the input unsupported (section 16) |
| proposed | a defect found while writing this file and not yet in `DESIGN.md` (section 19.2) |
| A, C | the item has parts in each tier; the row's notes and section 19 say which |
| — | not a behaviour of its own: a Python helper whose effect the named row covers, or Python-only tooling with no unityrs counterpart. The Python bindings (P3) decide whether to mirror it. |

**Level** is a verification level from `DESIGN.md` section 6: L0 detection, L1 container metadata, L2 typetree dicts, L2b TPK nodes, L3 object re-save, L4 file save, L5 pixels, mode and visible hash, L5b PNG bytes, L6 mesh numbers and OBJ text, L7 text exports, L8 audio, L9 extractor tree, L10 consumer shadow runs, L11 3D TRS. **L1 (env)** marks metadata at Environment scope (container paths after the preload pass, cross-file PPtrs): the canonical dump's format 1 leaves it out (`CANONICAL_DUMP.md` section 8), so it needs a format bump. **API** marks a Python-binding behaviour (a return type, an exception, a side effect) with no output of its own; it is tested through `unityrs-py` in P3.

**Status:**
- **verified P0 (Ln):** the oracle (`tools/parity/unitypy_dump.py`) records UnityPy's behaviour at level Ln in the canonical dump, `compare.py` diffs it, and the named self-test pins the oracle's side. The rabex spike, the candidate for the base layer, matched it on every record of the corpus game and UnityPy's samples with its one-hunk header fix (`docs/spikes/rabex.md` section 3). The production crate lands in the phase given after the semicolon.
- **pinned P0:** a test in `tools/parity/tests/` (named in the test column) runs UnityPy on a synthetic input built in code and pins what it does; no corpus is needed, and the canonical dump may not record it. The unityrs side lands in the phase given after the semicolon.
- **planned (Pn):** nothing checks it yet; it lands in phase Pn of `DESIGN.md` section 5.
- **n/a:** tier —.

**Test** names a self-test in `tools/parity/tests/` (module and test without its `test_` prefix), or says what the planned test compares.

## 1. The oracle stack

The pins are `DESIGN.md` section 3's. The image also carries these libraries, whose versions decide UnityPy's behaviour on the rows named (from `pip list` in `unityrs-dev`, 2026-10-28):

| library | version | decides |
|---|---|---|
| lz4 | 4.4.4 | LZ4/LZ4HC blocks (section 6), `CompressionHelper.compress_lz4` bytes |
| brotli | 1.2.0 | brotli WebFiles |
| pycryptodome | 3.23.0 | UnityCN key decryption (`ArchiveStorageManager.decrypt_key`) |
| numpy | 2.4.6 | the Sprite affine solve (`SpriteHelper.copy_triangle`), FMOD float-to-int16 conversion |
| pyfmodex | 0.7.2 | FMOD bindings under fmod_toolkit 0.1.3, which ships its own `libfmod` |
| fsspec | 2025.5.1 | `Environment(fs=...)` |
| attrs | 26.1.0 | the generated classes and `ObjectReader` |

TypeTreeGeneratorAPI is not installed, so `TypeTreeGenerator` raises `ImportError` in the oracle (section 10).

## 2. Package and entry points

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `UnityPy.load` | alias of `Environment` (`__init__.py:8`) | A | API | planned: binding | planned (P3) |
| `UnityPy.AssetsManager` | backward-compatible alias of `Environment` (`__init__.py:11`) | A | API | planned: binding | planned (P3) |
| `UnityPy.__version__` | `"1.25.2"` (`__init__.py:1`) | — | — | — | n/a |
| `UnityPy.set_assetbundle_decrypt_key` | re-export of `ArchiveStorageManager.set_assetbundle_decrypt_key` (`__init__.py:4-6`) | A | API | planned: binding | planned (P3) |
| `python -m UnityPy` | runs `cli.main` (`__main__.py:1-4`) | — | — | — | n/a |
| `cli.main` | argparse front end with one sub-command, `update_tpk` (`cli/__init__.py:6-24`) | — | — | — | n/a |
| `update_tpk.update_tpk` | downloads AssetRipper's TPK over HTTP and regenerates `classes/generated.py` (`cli/update_tpk.py:12-24`). unityrs loads a TPK from a path instead (`DESIGN.md` section 4, `unityrs-tpk`). | — | — | — | n/a |
| package re-exports | `files/__init__.py`, `helpers/__init__.py`, `streams/__init__.py`, `export/__init__.py`, `enums/__init__.py`, `classes/__init__.py`, `classes/legacy_patch/__init__.py`, `cli/__init__.py`: section 2.1 | A | API | planned: binding | planned (P3) |
| empty packages | `resources/__init__.py` (holds `lzma.tpk`), `tools/__init__.py` | — | — | — | n/a |

### 2.1 Package re-exports

What a package's `__init__.py` makes reachable from `import UnityPy` (`inventory.py` group `exports`). Each name is the same object as the class, function or table whose own row gives its tier; this table is its import path, which the Python bindings mirror. `UnityPy.helpers` and `UnityPy.export` re-export only their own submodules (their `__all__`; sections 22 and 15), and `export/__init__.py` leaves `ShaderConverter` out. `resources` and `tools` re-export nothing.

Every row is tier A at level API, test planned: binding, status planned (P3).

| package | source | members |
|---|---|---|
| `UnityPy` | `__init__.py:3-11`; `load` and `AssetsManager` are aliases of `Environment` | `Environment`, `load`, `AssetsManager`, `set_assetbundle_decrypt_key` |
| `UnityPy.files` | `files/__init__.py:1-5`; `UnityPy.files.BundleFile` is the class, not the module | `BundleFile`, `DirectoryInfo`, `File`, `ObjectReader`, `SerializedFile`, `WebFile` |
| `UnityPy.streams` | `streams/__init__.py:1-2` | `EndianBinaryReader`, `EndianBinaryWriter` |
| `UnityPy.enums` | `enums/__init__.py:1-17`; `AUDIO_TYPE_EXTEMSION` and `CommonString` are tables, the rest enums | `AUDIO_TYPE_EXTEMSION`, `ArchiveFlags`, `ArchiveFlagsOld`, `AudioCompressionFormat`, `AudioType`, `BuildTarget`, `ClassIDType`, `CommonString`, `CompressionFlags`, `FileType`, `GfxPrimitiveType`, `GraphicsFormat`, `PassType`, `SerializedPropertyType`, `ShaderCompilerPlatform`, `ShaderGpuProgramType`, `SpriteMeshType`, `SpritePackingMode`, `SpritePackingRotation`, `TextureDimension`, `TextureFormat` |
| `UnityPy.cli` | `cli/__init__.py:3`; the function, not the module | `update_tpk` |
| `UnityPy.classes.legacy_patch` | `classes/legacy_patch/__init__.py:1-8`: the generated classes it patches | `AudioClip`, `GameObject`, `Mesh`, `Renderer`, `Shader`, `Sprite`, `Texture2D`, `Texture2DArray` |
| `UnityPy.classes` | `classes/__init__.py:1-28`, through `from .generated import *`: the base classes, the math records and `generated.unitypy_define` | `ColorRGBA`, `Matrix3x4f`, `Matrix4x4f`, `Object`, `PPtr`, `Quaternionf`, `UnknownObject`, `Vector2f`, `Vector3f`, `Vector4f`, `float3`, `float4`, `unitypy_define` |
| `UnityPy.classes` | the generated classes (Appendix A) starting A | `AABB`, `ASTCImporter`, `AddedComponent`, `AddedGameObject`, `AimConstraint`, `AnchoredJoint2D`, `AndroidAssetPackImporter`, `AndroidDeviceFilterData`, `Animation`, `AnimationClip`, `AnimationClipBindingConstant`, `AnimationClipOverride`, `AnimationCurve`, `AnimationEvent`, `AnimationManager`, `Animator`, `AnimatorCondition`, `AnimatorController`, `AnimatorOverrideController`, `AnimatorState`, `AnimatorStateMachine`, `AnimatorStateTransition`, `AnimatorTransition`, `AnimatorTransitionBase`, `Annotation`, `AnnotationManager`, `AreaEffector2D`, `ArticulationBody`, `ArticulationDrive`, `AspectRatios`, `AssemblyDefinitionAsset`, `AssemblyDefinitionImporter`, `AssemblyDefinitionReferenceAsset`, `AssemblyDefinitionReferenceImporter`, `AssemblyJsonAsset`, `AssemblyJsonImporter`, `Asset`, `AssetBundle`, `AssetBundleFullName`, `AssetBundleInfo`, `AssetBundleManifest`, `AssetBundleScriptInfo`, `AssetDatabase`, `AssetDatabaseMetrics`, `AssetDatabaseV1`, `AssetImportInProgressProxy`, `AssetImporter`, `AssetImporterHashKey`, `AssetImporterLog`, `AssetImporter_ImportError`, `AssetInfo`, `AssetLabels`, `AssetMetaData`, `AssetServerCache`, `AssetStats`, `AssetTimeStamp`, `AttachmentIndexArray`, `AttachmentInfo`, `AudioBehaviour`, `AudioBuildInfo`, `AudioChorusFilter`, `AudioClip`, `AudioContainerElement`, `AudioDistortionFilter`, `AudioEchoFilter`, `AudioFilter`, `AudioHighPassFilter`, `AudioImporter`, `AudioImporterOutput`, `AudioListener`, `AudioLowPassFilter`, `AudioManager`, `AudioMixer`, `AudioMixerConstant`, `AudioMixerController`, `AudioMixerEffectController`, `AudioMixerGroup`, `AudioMixerGroupController`, `AudioMixerLiveUpdateBool`, `AudioMixerLiveUpdateFloat`, `AudioMixerSnapshot`, `AudioMixerSnapshotController`, `AudioRandomContainer`, `AudioResource`, `AudioReverbFilter`, `AudioReverbZone`, `AudioSource`, `AutoOffMeshLinkData`, `Avatar`, `AvatarBodyMask`, `AvatarConstant`, `AvatarMask`, `AvatarSkeletonMask`, `AvatarSkeletonMaskElement`, `Axes` |
| `UnityPy.classes` | the generated classes (Appendix A) starting B | `BaseAnimationTrack`, `BaseVideoTexture`, `Behaviour`, `BillboardAsset`, `BillboardRenderer`, `BitField`, `Blend1dDataConstant`, `Blend2dDataConstant`, `BlendDirectDataConstant`, `BlendShapeData`, `BlendShapeVertex`, `BlendTree`, `BlendTreeConstant`, `BlendTreeNodeConstant`, `BlobObject`, `BlockShaderContainer`, `BlockShaderErrors`, `BlockShaderImporter`, `BlockShaderSourceArtifact`, `BlockShaderSyntaxTree`, `BoneInfluence`, `BoneWeights4`, `BoxCollider`, `BoxCollider2D`, `BranchWindLevel`, `BrokenPrefabAsset`, `BufferBinding`, `BuildArchiveImporter`, `BuildInstructionImporter`, `BuildMetaDataImporter`, `BuildReport`, `BuildReportFile`, `BuildReportPackedAssetInfo`, `BuildReportScenesUsingAsset`, `BuildSettings`, `BuildStepInfo`, `BuildStepMessage`, `BuildSummary`, `BuildTargetSettings`, `BuildTextureStackReference`, `BuiltAssetBundleInfo`, `BuiltAssetBundleInfoSet`, `BuiltinShaderSettings`, `BuoyancyEffector2D`, `bitset` |
| `UnityPy.classes` | the generated classes (Appendix A) starting C | `C4DImporter`, `CGProgram`, `CachedAssetMetaData`, `CachedSpriteAtlas`, `CachedSpriteAtlasRuntimeData`, `Camera`, `Canvas`, `CanvasGroup`, `CanvasRenderer`, `CapsuleCollider`, `CapsuleCollider2D`, `Channel`, `ChannelInfo`, `CharacterController`, `CharacterInfo`, `CharacterJoint`, `Child`, `ChildAnimatorState`, `ChildAnimatorStateMachine`, `ChildMotion`, `CircleCollider2D`, `ClampVelocityModule`, `ClassInfo`, `ClassMethodInfo`, `Clip`, `ClipAnimationInfo`, `ClipAnimationInfoCurve`, `ClipMuscleConstant`, `Cloth`, `ClothAttachment`, `ClothConstrainCoefficients`, `ClothRenderer`, `ClothSphereColliderPair`, `CloudServiceHandlerBehaviour`, `CloudWebServicesManager`, `ClusterInput`, `ClusterInputManager`, `CollabEditorSettings`, `Collider`, `Collider2D`, `Collision`, `Collision2D`, `CollisionModule`, `ColorBySpeedModule`, `ColorModule`, `Component`, `ComponentPair`, `CompositeCollider2D`, `CompressedAnimationCurve`, `CompressedMesh`, `ComputeBufferCounter`, `ComputeShader`, `ComputeShaderBuiltinSampler`, `ComputeShaderCB`, `ComputeShaderImporter`, `ComputeShaderKernel`, `ComputeShaderKernelParent`, `ComputeShaderParam`, `ComputeShaderPlatformVariant`, `ComputeShaderResource`, `ComputeShaderVariant`, `Condition`, `ConditionConstant`, `ConfigSetting`, `ConfigurableJoint`, `ConstantBuffer`, `ConstantClip`, `ConstantForce`, `ConstantForce2D`, `ConstraintSource`, `ContentSummary`, `ControllerConstant`, `CrashReportManager`, `CrashReportingSettings`, `Cubemap`, `CubemapArray`, `CustomCollider2D`, `CustomDataModule`, `CustomRenderTexture` |
| `UnityPy.classes` | the generated classes (Appendix A) starting D | `D3D12DeviceFilterData`, `D3D12DeviceFilterLists`, `D3D12GraphicsJobsDeviceFilterData`, `DDSImporter`, `DataTemplate`, `DateTime`, `DefaultAsset`, `DefaultImporter`, `DefaultPreset`, `DefaultPresetList`, `DelayedCallManager`, `DeletedItem`, `DenseClip`, `Derived`, `DetailDatabase`, `DetailPatch`, `DetailPrototype`, `DeviceNone`, `DifferentMarshallingTestObject`, `DirectorGenericBinding`, `DirectorPlayer`, `DistanceJoint2D` |
| `UnityPy.classes` | the generated classes (Appendix A) starting E | `EdgeCollider2D`, `EditorBuildSettings`, `EditorExtension`, `EditorExtensionImpl`, `EditorProjectAccess`, `EditorSettings`, `EditorUserBuildSettings`, `EditorUserSettings`, `EffectConstant`, `Effector2D`, `EllipsoidParticleEmitter`, `EmbeddedNativeType`, `EmissionModule`, `EmptyObject`, `EnlightenRendererInformation`, `EnlightenSceneMapping`, `EnlightenSystemAtlasInformation`, `EnlightenSystemInformation`, `EnlightenTerrainChunksInformation`, `EntityId`, `Error`, `ExpandedData`, `ExposedReferenceTable`, `Expression`, `ExtensionPropertyValue`, `ExternalForcesModule` |
| `UnityPy.classes` | the generated classes (Appendix A) starting F | `FBXImporter`, `FakeComponent`, `FalloffTable`, `FastPropertyName`, `FixedJoint`, `FixedJoint2D`, `Flare`, `FlareElement`, `FlareLayer`, `FloatCurve`, `Font`, `ForceModule`, `FrictionJoint2D` |
| `UnityPy.classes` | the generated classes (Appendix A) starting G | `GISettings`, `GLTextureSettings`, `GUID`, `GUIDSerializer`, `GUIElement`, `GUILayer`, `GUIText`, `GUITexture`, `GameManager`, `GameObject`, `GameObjectRecorder`, `GenericBinding`, `GfxBlendState`, `GfxDepthState`, `GfxRasterState`, `GfxRenderTargetBlendState`, `GfxStencilState`, `GlobalGameManager`, `Google`, `Gradient`, `GradientNEW`, `GraphicsSettings`, `GraphicsStateCollection`, `GraphicsStateInfo`, `Grid`, `GridLayout`, `GroupConnection`, `GroupConstant` |
| `UnityPy.classes` | the generated classes (Appendix A) starting H | `Halo`, `HaloLayer`, `HaloManager`, `Hand`, `HandPose`, `Handle`, `Hash128`, `HeightMeshBVNode`, `HeightMeshData`, `Heightmap`, `HeightmapData`, `HierarchicalSceneData`, `HierarchyState`, `HingeJoint`, `HingeJoint2D`, `HoloLens`, `Human`, `HumanBone`, `HumanDescription`, `HumanGoal`, `HumanHandle`, `HumanLayerConstant`, `HumanPose`, `HumanPoseMask`, `HumanTemplate` |
| `UnityPy.classes` | the generated classes (Appendix A) starting I | `IConstraint`, `IHVImageFormatImporter`, `Image`, `ImportLog`, `ImportLog_ImportLogEntry`, `InheritVelocityModule`, `InitialModule`, `InputAxis`, `InputImportSettings`, `InputManager`, `InsightsSettings`, `InspectorExpandedState`, `IntPoint`, `InteractiveCloth`, `Item`, `int2_storage`, `int3_storage` |
| `UnityPy.classes` | the generated classes (Appendix A) starting J | `Joint`, `Joint2D`, `JointAngleLimit2D`, `JointAngleLimits2D`, `JointDrive`, `JointLimits`, `JointMotor`, `JointMotor2D`, `JointSpring`, `JointSuspension2D`, `JointTranslationLimits2D` |
| `UnityPy.classes` | the generated classes (Appendix A) starting K | `KTXImporter`, `Keyframe` |
| `UnityPy.classes` | the generated classes (Appendix A) starting L | `LOD`, `LODGroup`, `LODRenderer`, `LayerConstant`, `LayoutDataOne`, `LayoutDataThree`, `LayoutDataTwo`, `LeafInfoConstant`, `LensFlare`, `LevelGameManager`, `LibraryAssetImporter`, `LibraryRepresentation`, `LifetimeByEmitterSpeedModule`, `Light`, `LightBakingOutput`, `LightProbeData`, `LightProbeGroup`, `LightProbeOcclusion`, `LightProbeProxyVolume`, `LightProbes`, `LightingDataAsset`, `LightingDataAssetParent`, `LightingSettings`, `LightmapBakeMode`, `LightmapData`, `LightmapParameters`, `LightmapSettings`, `LightmapSnapshot`, `LightsModule`, `Limit`, `LineParameters`, `LineRenderer`, `LocalizationAsset`, `LocalizationImporter`, `LodSelectionCurve`, `LookAtConstraint`, `LowerResBlitTexture`, `Lumin` |
| `UnityPy.classes` | the generated classes (Appendix A) starting M | `MarshallingTestObject`, `MasterServerInterface`, `Material`, `MaterialImportOutput`, `MaterialInstanceSettings`, `MatrixParameter`, `MdFour`, `MemorySettings`, `Mesh`, `Mesh3DSImporter`, `MeshBlendShape`, `MeshBlendShapeChannel`, `MeshBlendShapeVertex`, `MeshCollider`, `MeshFilter`, `MeshLodInfo`, `MeshLodRange`, `MeshLodSubMesh`, `MeshParticleEmitter`, `MeshRenderer`, `MinMaxAABB`, `MinMaxCurve`, `MinMaxGradient`, `MipmapLimitSettings`, `ModelImporter`, `Module`, `MonoAssemblyImporter`, `MonoBehaviour`, `MonoImporter`, `MonoManager`, `MonoObject`, `MonoScript`, `Motion`, `MotionNeighborList`, `MovieImporter`, `MovieTexture`, `MultiArtifactTestImporter`, `MultiModeParameter`, `MultiplayerManager`, `MultiplayerRolesData` |
| `UnityPy.classes` | the generated classes (Appendix A) starting N | `NScreenBridge`, `NameToObjectMap`, `NamedObject`, `NativeFormatImporter`, `NativeObjectType`, `NativeType`, `NavMesh`, `NavMeshAgent`, `NavMeshAreaData`, `NavMeshAreas`, `NavMeshBuildDebugSettings`, `NavMeshBuildSettings`, `NavMeshData`, `NavMeshLayerData`, `NavMeshLayers`, `NavMeshObsolete`, `NavMeshObstacle`, `NavMeshParams`, `NavMeshProjectSettings`, `NavMeshSettings`, `NavMeshTileData`, `NetworkManager`, `NetworkView`, `NetworkViewID`, `NewAnimationTrack`, `Node`, `NoiseModule`, `NonAlignedStruct`, `NotificationManager` |
| `UnityPy.classes` | the generated classes (Appendix A) starting O | `ObjectRolePair`, `OcclusionArea`, `OcclusionCullingData`, `OcclusionCullingSettings`, `OcclusionPortal`, `OcclusionScene`, `Oculus`, `OffMeshLink`, `OffsetPtr`, `Output` |
| `UnityPy.classes` | the generated classes (Appendix A) starting P | `PPtrCurve`, `PPtrKeyframe`, `PVRImporter`, `PackageManifest`, `PackageManifestImporter`, `PackedAssets`, `PackedBitVector`, `PackingSettings`, `PanelRenderer`, `Parameter`, `ParentConstraint`, `ParserBindChannels`, `ParticleAnimator`, `ParticleEmitter`, `ParticleRenderer`, `ParticleSystem`, `ParticleSystemEmissionBurst`, `ParticleSystemForceField`, `ParticleSystemForceFieldParameters`, `ParticleSystemRenderer`, `PerLODSettings`, `PerformanceReportingManager`, `PerformanceReportingSettings`, `PhysicMaterial`, `Physics2DSettings`, `PhysicsJobOptions2D`, `PhysicsManager`, `PhysicsMaterial`, `PhysicsMaterial2D`, `PhysicsShape`, `PhysicsShapeGroup2D`, `PhysicsUpdateBehaviour2D`, `Pipeline`, `PlatformEffector2D`, `PlatformModuleSetup`, `PlatformSettings`, `PlatformSettingsData`, `PlatformShaderDefines`, `PlatformShaderSettings`, `PlayableDirector`, `PlayerSettings`, `PluginBuildInfo`, `PluginImportOutput`, `PluginImporter`, `PointEffector2D`, `Polygon2D`, `PolygonCollider2D`, `PolygonColliderBase2D`, `PositionConstraint`, `Prefab`, `PrefabImporter`, `PrefabInstance`, `PrefabModification`, `PreloadData`, `Preset`, `PresetManager`, `PresetType`, `PreviewAnimationClip`, `PreviewData`, `PreviewImporter`, `ProbeSetIndex`, `ProbeSetTetrahedralization`, `ProceduralMaterial`, `ProceduralMaterialInformation`, `ProceduralTexture`, `ProceduralTextureAssignment`, `Projector`, `PropertyModification`, `PropertyModificationsTargetTestNativeObject`, `PropertyModificationsTargetTestObject` |
| `UnityPy.classes` | the generated classes (Appendix A) starting Q | `QualitySetting`, `QualitySettings`, `QuaternionCurve` |
| `UnityPy.classes` | the generated classes (Appendix A) starting R | `RationalTime`, `RayTracingShader`, `RayTracingShaderBuiltinSampler`, `RayTracingShaderConstantBuffer`, `RayTracingShaderFunctionDesc`, `RayTracingShaderID`, `RayTracingShaderImporter`, `RayTracingShaderParam`, `RayTracingShaderPlatformVariant`, `RayTracingShaderReflectionData`, `RayTracingShaderResource`, `RayTracingShaderResources`, `RayTracingShaderVariant`, `RaycastCollider`, `RectTransform`, `Rectf`, `ReferencesArtifactGenerator`, `ReflectionProbe`, `RelativeJoint2D`, `RenderAs2D`, `RenderManager`, `RenderPassAttachment`, `RenderPassInfo`, `RenderSettings`, `RenderStateBlock`, `RenderStateInfo`, `RenderTexture`, `Renderer`, `RendererData`, `RendererFake`, `ResourceManager`, `ResourceManager_Dependency`, `Rigidbody`, `Rigidbody2D`, `RippleGroup`, `RootMotionData`, `RoslynAdditionalFileAsset`, `RoslynAdditionalFileImporter`, `RoslynAnalyzerConfigAsset`, `RoslynAnalyzerConfigImporter`, `RotationBySpeedModule`, `RotationConstraint`, `RotationModule`, `RuleSetFileAsset`, `RuleSetFileImporter`, `RuntimeAnimatorController`, `RuntimeInitializeOnLoadManager` |
| `UnityPy.classes` | the generated classes (Appendix A) starting S | `SBranchWindLevel`, `SParams`, `SWindGroup`, `SampleClip`, `SampleSettings`, `SamplerParameter`, `ScaleConstraint`, `Scene`, `SceneAsset`, `SceneDataContainer`, `SceneIdentifier`, `SceneObjectIdentifier`, `SceneRoots`, `SceneSettings`, `SceneVisibilityData`, `SceneVisibilityState`, `ScenesUsingAssets`, `ScriptMapper`, `ScriptableCamera`, `ScriptedImporter`, `SecondarySpriteTexture`, `SecondaryTextureSettings`, `SelectorStateConstant`, `SelectorTransitionConstant`, `SerializableManagedHost`, `SerializableManagedRefTestClass`, `SerializedCustomEditorForRenderPipeline`, `SerializedPass`, `SerializedPlayerSubProgram`, `SerializedProgram`, `SerializedProgramParameters`, `SerializedProperties`, `SerializedProperty`, `SerializedShader`, `SerializedShaderDependency`, `SerializedShaderFloatValue`, `SerializedShaderRTBlendState`, `SerializedShaderState`, `SerializedShaderVectorValue`, `SerializedStencilOp`, `SerializedSubProgram`, `SerializedSubShader`, `SerializedTagMap`, `SerializedTextureProperty`, `Shader`, `ShaderBindChannel`, `ShaderContainer`, `ShaderImporter`, `ShaderInclude`, `ShaderIncludeImporter`, `ShaderInfo`, `ShaderNameRegistry`, `ShaderVariantCollection`, `ShadowSettings`, `ShapeModule`, `SiblingDerived`, `SizeBySpeedModule`, `SizeModule`, `Skeleton`, `SkeletonBone`, `SkeletonBoneLimit`, `SkeletonMask`, `SkeletonMaskElement`, `SkeletonPose`, `SketchUpImportCamera`, `SketchUpImportData`, `SketchUpImportScene`, `SketchUpImporter`, `SkinnedCloth`, `SkinnedMeshRenderer`, `Skybox`, `SliderJoint2D`, `SnapshotConstant`, `SoftJointLimit`, `SoftJointLimitSpring`, `SortingGroup`, `SortingLayerEntry`, `SourceAssetIdentifier`, `SourceTextureInformation`, `SparseTexture`, `SpeedTreeImporter`, `SpeedTreeWind`, `SpeedTreeWindAsset`, `SpeedTreeWindConfig8`, `SpeedTreeWindConfig9`, `SphereCollider`, `SphericalHarmonicsL2`, `SplashScreenLogo`, `SplatDatabase`, `SplatPrototype`, `SpringJoint`, `SpringJoint2D`, `Sprite`, `SpriteAtlas`, `SpriteAtlasAsset`, `SpriteAtlasAssetData`, `SpriteAtlasData`, `SpriteAtlasDatabase`, `SpriteAtlasEditorData`, `SpriteAtlasImporter`, `SpriteBone`, `SpriteCustomDataEntry`, `SpriteCustomMetadata`, `SpriteData`, `SpriteMask`, `SpriteMetaData`, `SpriteRenderData`, `SpriteRenderer`, `SpriteShapeRenderer`, `SpriteSheetMetaData`, `SpriteTilingProperty`, `SpriteVertex`, `State`, `StateConstant`, `StateKey`, `StateMachine`, `StateMachineBehaviourVectorDescription`, `StateMachineConstant`, `StateRange`, `StaticBatchInfo`, `StreamInfo`, `StreamedClip`, `StreamedResource`, `StreamingController`, `StreamingInfo`, `StreamingManager`, `StructParameter`, `StyleSheetImporter`, `SubCollider`, `SubDerived`, `SubEmitterData`, `SubMesh`, `SubModule`, `SubPassDescriptor`, `SubstanceArchive`, `SubstanceEnumItem`, `SubstanceImporter`, `SubstanceInput`, `SubstanceValue`, `SurfaceEffector2D` |
| `UnityPy.classes` | the generated classes (Appendix A) starting T | `TagManager`, `TakeInfo`, `TargetJoint2D`, `Terrain`, `TerrainCollider`, `TerrainData`, `TerrainLayer`, `TestObjectVectorPairStringBool`, `TestObjectWithSerializedAnimationCurve`, `TestObjectWithSerializedArray`, `TestObjectWithSerializedMapStringBool`, `TestObjectWithSerializedMapStringNonAlignedStruct`, `TestObjectWithSpecialLayoutOne`, `TestObjectWithSpecialLayoutTwo`, `Tetrahedron`, `TextAsset`, `TextMesh`, `TextScriptImporter`, `Texture`, `Texture2D`, `Texture2DArray`, `Texture3D`, `TextureImportInstructions`, `TextureImportOutput`, `TextureImporter`, `TextureImporterPlatformSettings`, `TextureParameter`, `TextureParameters`, `TextureSettings`, `TicksPerSecond`, `TierGraphicsSettings`, `Tile`, `TileAnimationData`, `Tilemap`, `TilemapCollider2D`, `TilemapEditorUserSettings`, `TilemapRefCountedData`, `TilemapRenderer`, `TimeManager`, `TrailModule`, `TrailRenderer`, `Transform`, `TransformMaskElement`, `Transition`, `TransitionConstant`, `Tree`, `TreeInstance`, `TreePrototype`, `TriggerModule`, `TrueTypeFontImporter`, `TypeStats` |
| `UnityPy.classes` | the generated classes (Appendix A) starting U | `UAVParameter`, `UIRenderer`, `UVAnimation`, `UVModule`, `UnityAdsManager`, `UnityAdsSettings`, `UnityAnalyticsManager`, `UnityAnalyticsSettings`, `UnityConnectSettings`, `UnityPropertySheet`, `UnityPurchasingSettings`, `UnitySceneHandle`, `UnityTexEnv`, `UpdateZoneInfo` |
| `UnityPy.classes` | the generated classes (Appendix A) starting V | `VFXCPUBufferData`, `VFXCPUBufferDesc`, `VFXEditorSystemDesc`, `VFXEditorTaskDesc`, `VFXEntryExposed`, `VFXEntryExpressionValue`, `VFXEventDesc`, `VFXExposedMapping`, `VFXExpressionContainer`, `VFXField`, `VFXGPUBufferDesc`, `VFXInstanceSplitDesc`, `VFXLayoutElementDesc`, `VFXLayoutOffset`, `VFXManager`, `VFXMapping`, `VFXMappingTemporary`, `VFXPropertySheetSerializedBase`, `VFXRenderer`, `VFXRendererSettings`, `VFXShaderSourceDesc`, `VFXSystemDesc`, `VFXTaskDesc`, `VFXTemplate`, `VFXTemporaryGPUBufferDesc`, `VRSettings`, `ValueArray`, `ValueArrayConstant`, `ValueConstant`, `ValueDelta`, `VariableBoneCountWeights`, `VariantInfo`, `Vector3Curve`, `VectorParameter`, `VelocityModule`, `VersionControlSettings`, `VertexData`, `VertexLayoutInfo`, `VideoBuildInfo`, `VideoClip`, `VideoClipImporter`, `VideoClipImporterOutput`, `VideoClipImporterTargetSettings`, `VideoPlayer`, `VisualEffect`, `VisualEffectAsset`, `VisualEffectImporter`, `VisualEffectInfo`, `VisualEffectObject`, `VisualEffectResource`, `VisualEffectSettings`, `VisualEffectSubgraph`, `VisualEffectSubgraphBlock`, `VisualEffectSubgraphOperator`, `VulkanDeviceFilterLists`, `VulkanGraphicsJobsDeviceFilterData` |
| `UnityPy.classes` | the generated classes (Appendix A) starting W | `WebCamTexture`, `WheelCollider`, `WheelFrictionCurve`, `WheelJoint2D`, `WindZone`, `WorldAnchor`, `WorldParticleCollider` |
| `UnityPy.classes` | the generated classes (Appendix A) starting X | `xform` |

## 3. Environment and loading

Lines are in `environment.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Environment` constructor | each argument: a path that is a file with extension `.apk`/`.zip` (case-sensitive) goes to `load_zip_file`; another file sets `path` to its directory and loads it (a `.splitN` name through `load_files`); a directory sets `path` and loads it recursively; a string that is neither is ignored without an error; any other argument loads as a stream (`:54-70`). `path` defaults to the cwd for a local fs. A path is opened as a stream (`fs.open`, `:137`), so it is read with the stream reader's string rule, not the memory reader's (section 13). | A | L0 | planned: loader suite | planned (P1) |
| `Environment.file` | set only when exactly one file loaded (`:72-73`) | A | API | planned: binding | planned (P3) |
| `Environment.fs` | any fsspec filesystem (`:40`); `load_file` still tests local paths with `os.path` (`:119-128`) | A | API | planned: binding | planned (P3) |
| `Environment.load_files` | `load_assets` with `open` (`:75-77`) | A | L0 | planned: loader suite | planned (P1) |
| `Environment.load_folder` | every file under the folder, through `fs.walk` order (`:79-81`) | A | L0 | planned: loader suite | planned (P1) |
| `Environment.load` | loads each existing path and keys `files` by basename (`:83-87`); `load_file` has already keyed it by the path as given, so when that path is not its own basename the file sits in `files` twice and `Environment.objects` lists its objects twice; a bare name is keyed once | A | L0 | `test_parity_rows.py` load_keys_each_file_by_path_and_basename | pinned P0; impl P1 |
| `Environment.load_file` | a `.splitN` path merges its parts (`:111-116`); a name that exists as given is used as is, so a relative name is tried against the cwd first (`:119`); otherwise a relative name is joined to `path`, then `{file}.split0` is tried, then a case-insensitive walk (`find_sensitive_path`); a miss returns `None` silently (`:117-133`). A path is opened as a stream (`:137`, section 13). Detects, parses, keys `files` by the name as given (else the stream's `name`, else its hash), and registers a SerializedFile or raw reader as a CAB under its simplified name (`:139-160`). | A | L0 | `test_parity_rows.py` load_file_tries_the_cwd_first; the rest: planned loader suite | pinned P0 (the cwd rule); impl P1 |
| `Environment.load_zip_file` | opens a zip/apk and loads every member through `load_assets` (`:162-174`); raises `TypeError` on other inputs | A | L0 | planned: zip fixture | planned (P3) |
| `Environment.load_assets` | merges `.splitN` members once per base path, loads the rest by name (`:282-308`) | A | L0 | planned: split fixture | planned (P3) |
| `.splitN` merging | `split0`, `split1`, … concatenated up to the first missing part after `split0` (a missing `split0` is skipped, not a stop), at most 999 parts (`:89-98`); the regex `environment.reSplit`, `(.*?([^\/\\]+?))\.split\d+` (`:25`) | A | L0 | planned: split fixture | planned (P3) |
| `Environment.save` | writes each file with `is_changed` to `out_path/basename`, through its `save(packer=pack)` (`:176-184`) | A | L4 | planned: save suite | planned (P3) |
| `Environment.objects` | every object of every non-dependency SerializedFile, depth-first in `files` order (`:186-206`); two files with the same CAB name both count | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.container` | runs the preload pass of every SerializedFile in `cabs` once (`_build_container_index`, `:208-215`), then joins the containers of the non-dependency SerializedFiles in `cabs` (`:217-226`): one per lower-cased basename, the last one loaded winning, in the order the names were first registered. Two bundles holding a CAB of the same name (one bundle in two folders, two builds, a patch folder) contribute only the later one's container, while `Environment.objects` lists both (section 19.2) | A; C for same-named CABs | L1 (env) | `test_parity_rows.py` same_named_cabs_share_one_registry_entry (UnityPy's side); planned: env dump | pinned P0; impl P1 |
| `Environment.assets` | non-dependency SerializedFiles, depth-first (`:228-247`) | A | API | planned: binding | planned (P3) |
| `Environment.get` | `getattr` with a default (`:249-250`) | — | API | — | n/a |
| `Environment.register_cab` | keys `cabs` by `simplify_name` and invalidates the container index (`:252-264`): one registry for the whole Environment, so a later file with the same CAB name replaces an earlier one, from any bundle | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.get_cab` | case-insensitive basename lookup (`:266-280`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.find_file` | CAB registry, then `path/name`, then a walk of `path` built once, matched by exact path, then by simplified name (dead: it compares a bare name with the walk's full paths, `:338`, `:342`), then by plain suffix with no separator boundary, first on the paths and then on the simplified paths; raises `FileNotFoundError` (`:310-354`) | A | L1 (env) | planned: env dump | planned (P1) |
| `environment.simplify_name` | `ntpath.basename(name).lower()`; the docstring says it drops the extension, and it does not (`:357-363`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.typetree_generator` | `None` by default; `ObjectReader.generate_monobehaviour_node` reads it (`:34`) | A | L2 | planned: provider suite | planned (P3) |

## 4. File types and detection

Lines are in `helpers/ImportHelper.py`. `CANONICAL_DUMP.md` section 4.2 (`l0`) is the full rule list.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `FileType` | `AssetsFile` 0, `BundleFile` 1, `WebFile` 2, `ResourceFile` 9, `ZIP` 10 (`enums/FileType.py:4-9`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| `ImportHelper.check_file_type` | under 20 bytes `ResourceFile`; signature to NUL within 20 bytes, read with the reader the input gets (section 13): from bytes, 20 bytes with no NUL raise `Exception` "String not terminated", while from a path or a stream they read as a name and the file is a `ResourceFile`; bundle, web and `PK\x03\x04` signatures; under 128 bytes `ResourceFile`; gzip and brotli WebFiles; else a big-endian SerializedFile header sanity check (`:44-124`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1. The path and stream rule: `test_parity_rows.py` a_path_reads_strings_with_the_stream_rule, pinned P0 |
| eight-`0xFA` signature | compared as eight U+00FA characters, so the raw bytes never match (`:63`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| zip detection | only when the fifth byte is NUL (`:69`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P3 |
| gzip WebFile detection | decompresses the whole stream; a bad stream raises (`:77-83`). It reads through the shared reader and returns without putting it back to 0, so `WebFile` then reads its two-byte magic past the start, gets nothing, takes the file for plain, and raises `ValueError` "Invalid WebFile signature" on the gzip bytes (`files/WebFile.py:20-21`, `:40-42`): no gzip WebFile loads, from a path, from bytes or as a bundle entry. The oracle records `WebFile`, then a `parse` error (section 19.2) | A (detection); B (loading) | L0 | `test_dump.py` l0_detection_and_name_rules; `test_parity_rows.py` gzip_webfile_is_detected_and_never_parsed | verified P0 (L0); impl P3 |
| `ImportHelper.parse_file` | builds `SerializedFile`/`BundleFile`/`WebFile` by type; an `AssetsFile` named `.resS`, `.resource`, `.config`, `.xml` or `.dat` stays a raw reader; a failure prints and re-raises (`:127-156`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| `ImportHelper.find_sensitive_path` | case-insensitive path resolution, one `os.listdir` per part (`:159-173`) | A | API | planned: loader suite | planned (P1) |
| `ImportHelper.file_name_without_extension` | path without its extension (`:16-17`); unused inside UnityPy | — | — | — | n/a |
| `ImportHelper.list_all_files` | `os.walk` skipping `.git` (`:20-29`); unused inside UnityPy | — | — | — | n/a |
| `ImportHelper.find_all_files` | `list_all_files` filtered by substring (`:32-41`); unused inside UnityPy | — | — | — | n/a |

## 5. Containers

### 5.1 BundleFile

Lines are in `files/BundleFile.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `BundleFile` header | signature, u32 format, player and engine strings (`:36-39`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P1 |
| `UnityArchive` | the detector's signature list lacks it (`helpers/ImportHelper.py:60-66`), and its header fails the SerializedFile sanity check (the bytes `hive` read as version `0x68697665`, above 100), so through loading a `UnityArchive` file is a `ResourceFile` kept as a raw reader and registered as a CAB, with no error (L0 `ResourceFile`/`raw`). Only `BundleFile(...)` called directly raises `NotImplementedError` (`:41-42`) | A (loading); U (a direct bundle parse) | L0 | `test_parity_rows.py` unityarchive_loads_as_a_raw_file | pinned P0; impl P1 |
| unknown signature | raises `NotImplementedError` (`:50-51`) | A | L0 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L0); impl P1 |
| `BundleFile.read_web_raw` | UnityWeb/UnityRaw other than format 6: hash and crc from format 4, level table, LZMA for UnityWeb with an 8-byte size prefix (`:55-94`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures (UnityRaw 5 and 7); UnityWeb: planned fixture | verified P0 (L1) for UnityRaw; impl P3 |
| UnityWeb/UnityRaw format 6 | read with the UnityFS layout plus one byte after the flags (`:44-45`, `:104-106`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P3 |
| `BundleFile.read_fs` | UnityFS 6-8: sizes and flags (`:96-102`), blocks info compressed by the flags' codec (`:139-141`), block table and directory (`:144-165`) | A | L1 | `test_dump.py` samples (format 6), corpus dump (format 8) | verified P0 (L1); impl P1 |
| `ArchiveFlags` vs `ArchiveFlagsOld` | old below 2020, and below 2020.3.34, 2021.3.2 and 2022.1.1 on those lines (`:113-121`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `BlocksInfoAtTheEnd` | blocks info read from the file's end, data from after the header (`:134-137`) | A | L1 | planned: fixture (none in the samples or the corpus) | planned (P1) |
| 16-byte alignment after the header | format ≥ 7, or a 2019 engine at 2019.4.15 or later (`:126-131`) | A | L1 | corpus dump (format 8); the 2019.4.15 format-6 case: planned fixture | verified P0 (L1) at format ≥ 7; impl P1 |
| `BlockInfoNeedPaddingAtStart` | 16-byte alignment before the data, new flags only (`:170-171`) | A | L1 | corpus dump (flags `0x243`) | verified P0 (L1); impl P1 |
| `BlockInfo`, `DirectoryInfoFS` | namedtuples for the block table and directory (`:13-14`) | — | L1 | — | n/a |
| `BundleFile.decompress_data` | codec from `flags & 0x3F`; the decryptor runs on compressed blocks with flag `0x100` (`:489-521`) | A | L1 | `test_dump.py` samples; decryptor: section 12 | verified P0 (L1); impl P1 |
| `BundleFile.parse_version` | `UnityVersion.from_str(version_engine)`, falling back to `config.FALLBACK_UNITY_VERSION` when it does not match or its major is 0 (`:523-536`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| entries | each directory entry is detected and parsed in turn; a failing entry fails the whole bundle (`files/File.py:76-90`) | A | L0 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L0); impl P1 |
| `BundleFile.save`, `BundleFile.save_fs`, `BundleFile.save_web_raw` | section 18 | | | | |

### 5.2 WebFile, File and resources

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `WebFile` | gzip by magic (never reached through loading, which has moved the reader first: section 4, gzip WebFile detection), brotli by `brotli` at 0x20, else plain (`files/WebFile.py:20-37`); entry offsets and sizes are not checked against the data, and an entry past the end reads short (section 13); signature must start `UnityWebData` or `TuanjieWebData` (`:40-43`); entry names decoded as strict UTF-8 (`:53`). The dump records it as `kind: web`. | A; B for gzip | L1 | planned: WebFile fixtures (gzip, brotli, plain); the gzip failure: `test_parity_rows.py` gzip_webfile_is_detected_and_never_parsed | planned (P3) |
| `WebFile.save` | section 18 | | | | |
| `File` | base of the three containers: `files` dict, `is_changed`, `cab_file` `CAB-UnityPy_Mod.resS`, `parent`, `environment`, `name` as basename (`files/File.py:16-39`) | A | API | planned: binding | planned (P3) |
| `DirectoryInfo` | `(path, offset, size)` namedtuple (`files/File.py:13`) | — | L1 | — | n/a |
| `File.read_files` | slices each entry, parses it, registers SerializedFiles and raw readers as CABs, copies directory flags onto the child (`files/File.py:76-90`); an entry whose `offset + size` runs past the data is cut to the bytes that exist, with no error (`files/File.py:81`, section 13) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures; the short entry: `test_parity_rows.py` reads_past_the_end_return_short_data | verified P0 (L1); impl P1 |
| `File.get_assets` | SerializedFiles below the file, depth-first (`files/File.py:41-50`) | A | API | planned: binding | planned (P3) |
| `File.get_objects` | objects of every child; a raw-reader child is yielded itself (`files/File.py:65-74`) | A | API | planned: binding | planned (P3) |
| `File.get_filtered_objects` | `get_objects` filtered by `obj.type` (`files/File.py:52-63`) | A | API | planned: binding | planned (P3) |
| `File.container` | dict union of the children's containers (`files/File.py:123-125`) | A | L1 (env) | planned: env dump | planned (P1) |
| `File.get`, `File.keys`, `File.items`, `File.values` | dict-style access to `files` (`files/File.py:127-140`) | — | API | — | n/a |
| `File.mark_changed` | marks the file and every parent (`files/File.py:145-148`) | A | L4 | planned: save suite | planned (P3) |
| `File.get_writeable_cab` | section 18 | | | | |
| `ResourceReader.get_resource_data` | looks up the CAB by basename, then `{name}.resource`, `{name}.assets.resS`, `{name}.resS`, in the Environment's one registry (`Environment.register_cab`), not in the asking file's own bundle, so when two bundles hold a CAB of the same name the later one's `.resS` supplies the data for both (section 19.2); on a miss calls `SerializedFile.load_dependencies` with those names (which loads the file's externals as non-dependencies, section 7.2) and looks again; raises `FileNotFoundError` (`helpers/ResourceReader.py:10-32`). `load_dependencies` swallows only `FileNotFoundError` (`files/SerializedFile.py:345-349`), so a name the loader cannot open otherwise escapes: an empty name is joined to `Environment.path`, a directory, and raises `IsADirectoryError` | A; C for same-named CABs | L5 | `test_parity_rows.py` same_named_cabs_share_one_registry_entry, an_empty_texture_follows_an_empty_stream_path; planned: `.resS` texture fixture | pinned P0; impl P1 |
| `.resS` read | `reader.Position = offset; read_bytes(size)` on the CAB (`helpers/ResourceReader.py:36-38`); a range past the end returns the bytes that exist, with no error (section 13) | A | L5 | `test_parity_rows.py` reads_past_the_end_return_short_data; planned: `.resS` texture fixture | pinned P0; impl P1 |

## 6. Compression codecs

Lines are in `helpers/CompressionHelper.py`. The bundle codec is `flags & 0x3F` as `CompressionFlags` (`enums/BundleFile.py:4-9`).

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| NONE (0) | identity (`:239`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P1 |
| LZMA (1) | 5-byte props, raw LZMA1, no size check: UnityFS ignores the stored size (`:17-44`, `:240`). See `docs/spikes/lzma.md` for truncated and over-long blocks. | A | L1 | `test_dump.py` samples; `docs/spikes/lzma.md` fixtures | verified P0 (L1); impl P1 |
| LZ4 (2), LZ4HC (3) | `lz4.block.decompress(data, stored_size)` (`:83-94`, `:241-242`): the stored size is the buffer's capacity, not a check. Output shorter than it is kept as is, with no error, and every later entry offset shifts; output that does not fit, or a stored size of 0, raises `LZ4BlockError`. The same holds for an LZ4 blocks-info (`files/BundleFile.py:141`) | A | L1 | `test_dump.py` samples; `test_parity_rows.py` lz4_stored_size_is_a_capacity | verified P0 (L1); impl P1 |
| LZHAM (4) | raises `NotImplementedError` (`:234-235`, `:243`) | U | L1 | planned: error record | planned (P1) |
| other codec values | raise `ValueError` "Unknown compression" (`files/BundleFile.py:515-521`) | A | L1 | planned: error record | planned (P1) |
| gzip | `gzip.decompress` (`:133-142`); WebFile only. Detection already decompresses the whole stream, and leaves the reader where it stopped, so no gzip WebFile loads (section 4); unityrs loads them, checked against `gzip.decompress` and the plain WebFile layout (section 19.2) | B | L0, L1 | `test_dump.py` l0_detection_and_name_rules (L0); `test_parity_rows.py` gzip_webfile_is_detected_and_never_parsed; WebFile fixture (L1) | verified P0 (L0); impl P3 |
| brotli | `brotli.decompress` (`:109-118`); WebFile only | A | L1 | planned: brotli WebFile fixture | planned (P3) |
| `CompressionHelper.decompress_lzma` | `read_decompressed_size` skips an 8-byte size, for UnityWeb (`:43`) | A | L1 | `test_dump.py` samples (UnityFS); UnityWeb: planned fixture | verified P0 (L1) for UnityFS; impl P1 |
| `CompressionHelper.decompress_lz4` | as LZ4 above | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `CompressionHelper.decompress_brotli`, `CompressionHelper.decompress_gzip` | as above | A | L1 | as above | as above |
| `CompressionHelper.decompress_lzham` | as LZHAM above | U | L1 | planned: error record | planned (P1) |
| `CompressionHelper.compress_lzma` | props `0x5D`, dictionary 8 MiB, lc 3 lp 0 pb 2, BT4, nice_len 123, optional u64 size (`:47-79`) | A | L4 | planned: save suite | planned (P3) |
| `CompressionHelper.compress_lz4` | `lz4.block.compress` HC level 9, no size prefix (`:97-105`); used for LZ4 and LZ4HC | A | L4 | planned: save suite | planned (P3) |
| `CompressionHelper.compress_gzip`, `CompressionHelper.compress_brotli` | library defaults (`:121-155`): brotli at quality 11, and gzip at level 9 with the current time as the header's mtime, so two gzip saves of one file differ | A | L4 | planned: save suite (after decompression) | planned (P3) |
| `CompressionHelper.DECOMPRESSION_MAP`, `CompressionHelper.COMPRESSION_MAP`, `CompressionHelper.COMPRESSION_CHUNK_SIZE_MAP` | the codec tables by `CompressionFlags` (`:238-258`): LZ4HC reads and writes as LZ4 HC, chunks are 128 KiB for LZ4 and LZ4HC and `0xFFFFFFFF` otherwise. They are looked up on every block (`files/BundleFile.py:515-518`), so adding an entry is how a UnityPy user registers a custom codec; unityrs's codec registry is the counterpart (`DESIGN.md` section 4) | A | API | planned: codec-registry binding | planned (P3) |
| `CompressionHelper.GZIP_MAGIC`, `CompressionHelper.BROTLI_MAGIC` | `1f 8b` at offset 0 and `brotli` at `0x20`, the WebFile sniffs (`:12-13`, `files/WebFile.py:20-37`); through loading the gzip sniff never matches, because detection has moved the reader (section 4) | A | L1 | planned: WebFile fixtures | planned (P3) |
| `CompressionHelper.chunk_based_compress` | LZ4/LZ4HC in 128 KiB chunks, LZMA in one; a chunk that grows is stored with its codec bits cleared (`:158-231`, sizes `:253-258`) | A | L4 | planned: save suite | planned (P3) |

## 7. SerializedFile

### 7.1 Version gates

Every branch on `header.version`. Lines are in `files/SerializedFile.py` unless named. The canonical dump records each one at L1 (`CANONICAL_DUMP.md` section 4.2). P0 exercises format 17 (UnityPy's samples), format 22 (the corpus game) and the synthetic formats 15, 16 and 22 that `tests/synth.py` builds; a gate only those cannot reach is planned for P4's Editor-built fixtures, though the oracle already records it.

| version | gate | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|---|
| all | header | four big-endian u32: metadata size, file size, version, data offset (`:32-38`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| < 9 | endian byte | read at `file_size - metadata_size`, at the end (`:265-267`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 9 | endian, reserved | endian byte and 3 reserved bytes after the header (`:257-259`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 22 | large header | u32 metadata size, i64 file size, i64 data offset, i64 unknown (`:260-264`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 7 | version string | read and passed to `set_version` (`:271-273`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| < 7 | no version string | `unity_version` stays `"2.5.0f5"` (`:239`) and `version` is never set, so anything that reads it raises `AttributeError` | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 8 | target platform | i32 as `BuildTarget`, which keeps unknown values (`:275-277`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 13 | `enable_type_tree` | bool (`:279-280`); below 13 every type carries a tree | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 16 | type `is_stripped_type` | bool (`:122-123`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 17 | type `script_type_index` | i16 (`:125-126`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 13 | `script_id`, `old_type_hash` | 16-byte script id for ref types with a script index, class id < 0 below 16, or class 114 from 16; then the 16-byte type hash (`:128-135`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| 10, ≥ 12 | blob typetree | `TypeTreeNode.parse_blob` (`:138-139`), only when the file's tree flag is true (`:137`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| < 10, 11 | old typetree | `TypeTreeNode.parse`, field by field (`:140-141`), only when the file's tree flag is true (`:137`; below 13 it always is) | A | L1 | planned: Editor fixture | planned (P4) |
| 2 | old node `variable_count` | read only at version 2 (`helpers/TypeTreeNode.py:98`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≠ 3 | old node `index`, `meta_flag` | absent at version 3 (`helpers/TypeTreeNode.py:99`, `helpers/TypeTreeNode.py:102`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 19 | blob `ref_type_hash` | node struct `hBBIIiii` gains `Q` (`helpers/TypeTreeNode.py:331-347`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| ≥ 21 | type dependencies, ref type names | inside the tree flag, after the tree: plain types read an i32 array, ref types read class, namespace and assembly (`:143-149`, nested in `:137`). A file with the flag false (common in shipped bundles) has neither, and `type_dependencies` stays `None`; a parser that reads the array anyway misreads every later byte | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy; the flag-off case: `test_parity_rows.py` type_dependencies_need_the_tree_flag | verified P0 (L1); impl P1 |
| 7-13 | `big_id_enabled` | i32 (`:286-288`) | A | L1 | planned: Editor fixture | planned (P4) |
| < 14 | path id | i32, or i64 with `big_id_enabled` (`files/ObjectReader.py:73-76`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 14 | path id | align 4, then i64 (`files/ObjectReader.py:77-79`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| ≥ 22 | byte start | i64, else u32; plus `data_offset` (`files/ObjectReader.py:81-86`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| < 16 | object type | the i32 type id is followed by a u16 class id; the type is the first whose class id equals the i32, and the object's class id is the u16 (`files/ObjectReader.py:89-97`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| ≥ 16 | object type | `types[type_id]` with Python indexing (`files/ObjectReader.py:98-101`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| < 11 | `is_destroyed` | u16 (`files/ObjectReader.py:106-107`) | A | L1 | planned: Editor fixture | planned (P4) |
| 11-16 | object script index | i16 written back into the shared type (`files/ObjectReader.py:109-112`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| 15, 16 | `is_stripped` | signed byte (`files/ObjectReader.py:115-116`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| ≥ 11 | script types | i32 count of `LocalSerializedObjectIdentifier` (`:298-300`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 14 | script identifier | align 4, then i64 (`:50-52`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| < 14 | script identifier | i32 (`:48-49`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 6 | external `temp_empty` | string (`:79-80`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 5 | external `guid`, `type` | 16 bytes and i32 (`:81-83`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 20 | `ref_types` | count and ref `SerializedType`s (`:306-308`); below 20 the attribute is never set | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 5 | `userInformation` | string (`:313-314`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| 23-26 | HEAD only | section 20 | HEAD | L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| < 6, < 5, < 8, < 13 | absent fields | `temp_empty`, `guid`/`type`, platform and the tree flag keep their defaults (`:79-84`, `:240-241`) | A | L1 | planned: Editor fixture | planned (P4) |
| any | big-endian files | `header.endian` drives every read after the header (`:269`) | A | L1, L2 | `test_unitypy_boost.py` big_endian_reads_the_same_values (values only) | verified P0 (L2) for values; files planned (P4) |

### 7.2 Other SerializedFile behaviour

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `SerializedFile` | parses header, types, objects, scripts, externals, ref types and user information (`files/SerializedFile.py:235-324`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `SerializedFileHeader` | attrs class for the header (`files/SerializedFile.py:23-38`) | — | L1 | — | n/a |
| `LocalSerializedObjectIdentifier` | script type entry; `write` mirrors the read (`files/SerializedFile.py:41-60`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `FileIdentifier` | external entry; `name` is the path's basename (`files/SerializedFile.py:63-94`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `SerializedType` | type record (`files/SerializedFile.py:97-149`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `SerializedType.nodes` | alias of `node` (`files/SerializedFile.py:200-203`) | — | API | — | n/a |
| `SerializedType.write` | section 18 | | | | |
| object table collapse | `objects` is a dict by path id: a repeated id keeps its first position and last entry (`files/SerializedFile.py:291-295`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `SerializedFile.files` | alias of `objects` (`files/SerializedFile.py:225-233`) | — | API | — | n/a |
| `SerializedFile.set_version` | empty or `0.0.0` takes the parent bundle's `version_engine`, then the fallback (`files/SerializedFile.py:351-360`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| AssetBundle at load | the first AssetBundle object is parsed while loading and builds the container; a broken one fails the file (`files/SerializedFile.py:316-324`) | A | L1 | `test_dump.py` samples (container); the failing path: planned fixture | verified P0 (L1); impl P1 |
| `SerializedFile.container` | the file's `ContainerHelper` (`files/SerializedFile.py:326-328`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `config.SERIALIZED_FILE_PARSE_TYPETREE` | `False` turns `_enable_type_tree` off only after the types and ref types, trees included, were read (`files/SerializedFile.py:284`, `:308`, `:310-311`, `config.py:12`), so despite its docstring it saves no load time and reading is unchanged (`ObjectReader` still uses each type's tree, `files/ObjectReader.py:296-297`); its one effect is on `SerializedFile.save`, which then writes the tree flag as false and no trees (`files/SerializedFile.py:179`, `:405`) | A | L4 | planned: config suite | planned (P3) |
| `SerializedFile.load_dependencies` | loads every external by path, then the optional extra names, ignoring `FileNotFoundError` for the extras (`files/SerializedFile.py:330-349`); `load_file(path, True)` passes `True` as `parent`, not `is_dependency` (`environment.py:100-106`), so what it loads counts as a non-dependency and joins `Environment.objects` and `Environment.container`; section 19.2 | A | L1 (env) | planned: env dump | planned (P1) |
| `SerializedFile.read_string` | common-string lookup from `enums/CommonString.py`; never called (`files/SerializedFile.py:496-503`) | — | — | — | n/a |
| `SerializedFile._cache` | per-file image cache for sprites (`files/SerializedFile.py:246-250`); see Tier C, Sprite cache key | C | L5 | planned: divergence test | planned (P1) |
| `SerializedFile.get_writeable_cab`, `SerializedFile.save` | section 18 | | | | |

## 8. ObjectReader

Lines are in `files/ObjectReader.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `ObjectReader` | attrs record of one object table entry; `type` is a `ClassIDType`, extendable to unknown ids (`:34-50`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `ObjectReader.from_reader` | the object table row, per version (section 7.1; `:67-130`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `ObjectReader.version`, `ObjectReader.version2`, `ObjectReader.platform` | the file's packed engine version, its format version and its `BuildTarget` (`:52-62`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.parse_as_dict` | `read_typetree(wrap=False)` (`:313-314`): the dict model of `CANONICAL_DUMP.md` section 3 | A | L2 | `test_unitypy_boost.py` (all); `test_dump.py` samples | verified P0 (L2); impl P1 |
| `ObjectReader.parse_as_object` | `read_typetree(wrap=True)` (`:310-311`): object mode, section 9.2 | A | API | planned: object-mode suite | planned (P3) |
| `ObjectReader.read_typetree` | node from `nodes=` or the type or the TPK; reads from `byte_start`; UnityPyBoost when `byte_size > 0`; `check_read` raises `ValueError` on a size mismatch (`:244-262`, `helpers/TypeTreeHelper.py:146-156`) | A | L2 | `test_unitypy_boost.py` error_kinds, check_read_values | verified P0 (L2); impl P1 |
| `ObjectReader.read` | `read_typetree(wrap=True)` (`:217-218`) | A | API | planned: object-mode suite | planned (P3) |
| `nodes=` override | a `TypeTreeNode`, or a list of node dicts built with `TypeTreeNode.from_list`; anything else raises `ValueError` (`:285-294`) | A | L2 | planned: provider suite | planned (P3) |
| TPK fallback | no embedded tree: `Tpk.get_typetree_node(class_id, version)` (`:296-299`) | A | L2b | planned: stripped fixture | planned (P3) |
| no tree at all | the TPK fallback raises: `KeyError` for a class id the TPK lacks, `ValueError` "Could not find exact version" for a version before the class's first entry or one where the TPK marks the class removed (`helpers/Tpk.py:472-481`), and `AssertionError` for a class with no release tree, such as the abstract EditorExtension (`helpers/Tpk.py:64`). The `TypeTreeError` "There are no TypeTree nodes" (`:305-306`) is unreachable: the TPK returns a node or raises, and a node is always truthy. The oracle records each as error kind `no_typetree` (`unitypy_dump.py` object_record) | A | L2 | `test_parity_rows.py` no_tree_raises_the_tpk_error | pinned P0; impl P1 |
| `ObjectReader.save_typetree` | writes the value with `write_typetree` in the file's endian, then `set_raw_data` (`:264-276`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |
| `ObjectReader.patch` | alias of `save_typetree` (`:316-322`) | A | L3 | as above | as above |
| `ObjectReader.set_raw_data` | replaces the data and marks the file changed (`:181-184`) | A | L4 | planned: save suite | planned (P3) |
| `ObjectReader.get_raw_data` | `byte_size` bytes from `byte_start`, position restored (`:278-283`) | A | L3 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L3); impl P1 |
| `ObjectReader.write` | the object table row for `save`; data from `set_raw_data` or the original bytes (`:132-179`) | A | L4 | planned: save suite | planned (P3) |
| `ObjectReader.peek_name` | reads the tree up to the first `m_Name`/`name` child; the peek node is cached process-wide by the root's `(m_Name, m_Type, m_Version)` (`:189-197`, `helpers/TypeTreeNode.py:190-211`) | A | API | planned: peek suite | planned (P3) |
| `ObjectReader.container` | builds the Environment's container index, then looks the object up by path id alone in its own file's `path_dict` (`:199-204`) | A | L1 (env) | planned: env dump | planned (P1) |
| `ObjectReader.get_class` | `ClassIDTypeToClassMap.get(type)`, `None` for ids without a class (`:186-187`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.dump_typetree_structure` | `TypeTreeNode.dump_structure` text (`:236-242`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.Position`, `ObjectReader.reset`, `ObjectReader.get` | reader position, seek to `byte_start`, `getattr` with default (`:206-221`) | — | API | — | n/a |
| unknown attributes | `__getattr__` forwards to the reader and returns `None` for anything else (`:223-225`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.parse_monobehaviour_head` | reads the TPK's MonoBehaviour tree without `check_read` (`:325-330`) | A | L2 | planned: provider suite | planned (P3) |
| `ObjectReader.generate_monobehaviour_node` | dereferences `m_Script` and asks `Environment.typetree_generator` for `{namespace}.{class}`; raises `ValueError` without a generator or a node, which `_get_typetree_node` swallows (`:332-350`, `:300-304`) | A | L2 | planned: provider suite | planned (P3) |

## 9. Typetrees

### 9.1 Reading and writing values

Lines are in `helpers/TypeTreeHelper.py` unless named. `CANONICAL_DUMP.md` sections 3 and 7 give the full dict model.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TypeTreeHelper.read_typetree` | UnityPyBoost for `byte_size > 0`, else the pure-Python reader from the reader's position (`:123-158`) | A | L2 | `test_unitypy_boost.py` (all) | verified P0 (L2); impl P1 |
| `UnityPyBoost.read_typetree` | the C reader: lists for arrays, tuples for pairs, bytes for TypelessData, surrogate-escaped strings, float32 widened, signalling NaN quieted (`CANONICAL_DUMP.md` section 7) | A | L2 | `test_unitypy_boost.py` float_bits, shapes | verified P0 (L2); impl P1 |
| `UnityPyBoost.TypeTreeNode` | the C node type `helpers/TypeTreeNode.py` subclasses when Boost is present (`helpers/TypeTreeNode.py:25-49`) | — | L1 | — | n/a |
| `TypeTreeHelper.read_value` | pure-Python reader: primitives, pairs as tuples, ReferencedObject, vectors, classes, registry scope (`:182-263`) | A | L2 | `test_unitypy_boost.py` pure_python_path_returns_tuples | verified P0 (L2); impl P1 |
| `TypeTreeHelper.read_value_array` | pure-Python arrays of aligned elements: tuples, 16-bit signedness swap, per-array alignment (`:266-361`) | A | L2 | `test_unitypy_boost.py` pure_python_path_swaps_16_bit_signedness | verified P0 (L2); impl P1 |
| ManagedReferencesRegistry scope | the first registry child starts a scope that skips every other registry until its class ends (`:225-231`) | A | L2 | `test_unitypy_boost.py` registry_scope_is_the_class_that_holds_it | verified P0 (L2); impl P1 |
| ReferencedObject | `data` typed by `ref_types` on `(class, ns, asm)`; empty class omits `data` (`:197-206`), on both reading paths; the writer then cannot re-save the value (`TypeTreeHelper.write_value`) | A | L2 | `test_unitypy_boost.py` referenced_objects_and_the_registry_scope | verified P0 (L2); impl P1 |
| `TypeTreeHelper.get_ref_type_node` | the pure-Python rule: raises `ValueError` "SerializedFile has no ref_types" when `ref_types` is missing or empty, before it looks at the class, then `None` for an empty class, then `ValueError` on an unknown type (`:88-109`). UnityPyBoost (L2, `byte_size > 0`) tests the empty class first and raises only when `ref_types` is missing, not when it is an empty list (`UnityPyBoost/TypeTreeHelper.cpp:592-643`), so a v20+ file with no ref types and a null reference reads on Boost and raises on the pure-Python path | A | L2 | `test_unitypy_boost.py` error_kinds; the empty list: `test_parity_rows.py` ref_type_lookup_differs_on_an_empty_list | verified P0 (L2); impl P1 |
| `TypeTreeHelper.write_typetree` | the pure-Python writer, dict or object (`:161-179`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |
| `TypeTreeHelper.write_value` | aligns after every element whose node carries the align flag, which its reader does not mirror (`:407-458`). A ReferencedObject of empty class has no `data` (the reader skipped it), but the writer still looks it up: `get_ref_type_node` returns `None` and `value["data"]` raises `KeyError` (`:422-426`), so the value UnityPy parsed cannot be re-saved (section 19.2) | A; B for the empty-class reference | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element; `test_parity_rows.py` an_empty_managed_reference_does_not_resave | verified P0 (L3); impl P3 |
| `TypeTreeHelper.metaflag_is_aligned` | `meta_flag & 0x4000` (`:364-365`) | — | L2 | — | n/a |
| `TypeTreeConfig` | `as_dict`, `assetsfile`, `has_registry` (`:78-85`) | — | L2 | — | n/a |
| `TypeTreeHelper.get_annotation_keys` | a class's annotation names (`:112-120`) | — | API | — | n/a |
| `TypeTreeHelper.clean_name` | `(int&)` prefix and `?` suffix dropped, ` .:-[]` to `_`, `pass`/`from` get `_`, a leading digit (`str.isdigit`, so `²` and `١` too) gets `x` (`:368-378`); unlike the next row it fails on `""`. Object mode does not use it (section 9.2) | — | API | — | n/a |
| `TypeTreeNode.clean_name` | the same, returning `""` for `""` (`helpers/TypeTreeNode.py:350-363`); its comment says it is kept in sync with UnityPyBoost, but UnityPyBoost tests only an ASCII leading digit (`UnityPyBoost/TypeTreeHelper.cpp:59`): `²ab` stays `²ab` there and becomes `x²ab` here. With UnityPyBoost present a node's `_clean_name` comes from the C++ rule; this function is the fallback without it | A | API | `test_parity_rows.py` object_mode_names_use_the_boost_rule | pinned P0; impl P3 |

### 9.2 Object mode

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| class lookup | `getattr(classes, m_Type, UnknownObject)` per class node; `PPtr<...>` nodes become `PPtr` (`helpers/TypeTreeHelper.py:233-258`) | A | API | planned: object-mode suite | planned (P3) |
| attribute names | each child node's `_clean_name` (`helpers/TypeTreeHelper.py:231`), which UnityPyBoost computes with its C++ rule: only an ASCII digit gets the `x` prefix (`UnityPyBoost/TypeTreeHelper.cpp:59`), so a field `²ab` is the attribute `²ab` and `2cd` is `x2cd` | A | API | `test_parity_rows.py` object_mode_names_use_the_boost_rule; planned: object-mode suite | pinned P0; impl P3 |
| missing fields | an `UnknownObject` carrying the node (`helpers/TypeTreeHelper.py:244-249`) | A | API | planned: object-mode suite | planned (P3) |
| extra fields | set as attributes after construction (`helpers/TypeTreeHelper.py:250-256`) | A | API | planned: object-mode suite | planned (P3) |
| pure-Python class arrays | the same rules per element (`helpers/TypeTreeHelper.py:314-357`) | A | API | planned: object-mode suite | planned (P3) |

### 9.3 TypeTreeNode

Lines are in `helpers/TypeTreeNode.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TypeTreeNode` | node with `m_Level`, `m_Type`, `m_Name`, `m_ByteSize`, `m_Version`, `m_Children`, `m_TypeFlags`, `m_VariableCount`, `m_Index`, `m_MetaFlag`, `m_RefTypeHash` (`:52-64`, `:70`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `TypeTreeNode.parse` | old format, stack-based, version gates of section 7.1 (`:78-109`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P4 |
| `TypeTreeNode.parse_blob` | blob rows under a hidden root; only the root's first child is kept; offsets with the high bit index the TPK's whole common-string table, and an offset missing from it becomes its decimal text (`:111-154`, `:129`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| `TypeTreeNode.from_list` | builds a tree from node dicts or nodes (`:156-188`). For dicts it needs `m_Level`, `m_Type` and `m_Name` in the first, and decides the defaults from the first alone: when the first lacks `m_ByteSize` or `m_Version`, every dict gets 0 for it (and one that has it raises `TypeError` for a repeated argument); when the first has it, a later dict without it raises `TypeError` (`:164-172`) | A | API | `test_parity_rows.py` tpk_version_lookup_and_from_list; planned: provider suite | pinned P0; impl P3 |
| `TypeTreeNode.traverse` | pre-order (`:71-76`) | A | API | planned: binding | planned (P3) |
| `TypeTreeNode.get_name_peek_node` | see `ObjectReader.peek_name` (`:190-211`) | A | API | planned: peek suite | planned (P3) |
| `TypeTreeNode.dump` | old-format writer; writes the root's fields for every node (`:213-235`); see section 18, pre-v10 writing | C | L4 | planned: divergence test | planned (P3) |
| `TypeTreeNode.dump_blob` | blob writer; common strings by offset, except the one at offset 0 (`:237-278`) | A, C | L4 | planned: save suite; divergence test | planned (P3) |
| `TypeTreeNode.dump_structure` | AssetRipper-style text dump (`:280-288`) | A | API | planned: binding | planned (P3) |
| `TypeTreeNode.to_dict`, `TypeTreeNode.to_dict_list` | non-`None` fields, and the pre-order list (`:290-299`) | A | API | planned: binding | planned (P3) |
| equality | fields and children (`:301-302`) | — | API | — | n/a |
| `TypeTreeNode.get_common_strings` | the TPK's common strings, all of them for `version=None` (the only call), offsets by cumulative length (`:308-328`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |

## 10. TPK, typetree providers and config

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `resources/lzma.tpk` | AssetRipper's TPK, LZMA, loaded at import (`helpers/Tpk.py:20-39`, `:484`). unityrs loads it from a path (`DESIGN.md` section 4, `unityrs-tpk`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `Tpk.init` | reads the bundled file (`helpers/Tpk.py:20-39`) | — | L2b | — | n/a |
| `Tpk.get_typetree_node` | `ClassInformation[class_id]` (a `KeyError` for unknown ids), the versioned class through `Tpk.get_item_for_version`, which raises before it can return `None`, so the `None` check after it is dead (`helpers/Tpk.py:49-51`), cached by `(class_id, version)` (`helpers/Tpk.py:42-55`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `Tpk.generate_node` | the release root node, pre-order, `m_Index` counted, no type flags (`helpers/Tpk.py:58-88`); a class with no release root (an abstract class) fails an `assert` (`helpers/Tpk.py:64`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `Tpk.get_item_for_version` | the last entry whose version ≤ the target (`helpers/Tpk.py:472-481`); raises `ValueError` "Could not find exact version" when the target is below every entry, and when the entry it lands on holds `None` (the TPK's mark for a class removed in that version) | A | L2b | `test_parity_rows.py` tpk_version_lookup_and_from_list, no_tree_raises_the_tpk_error; planned: TPK node-list diff | pinned P0; impl P3 |
| `TpkFile` | magic `TPK*`, version 1, NONE/LZ4/LZMA/brotli (`helpers/Tpk.py:145-197`) | A | L2b | planned: TPK loader test | planned (P3) |
| `TpkCompressionType`, `TpkDataType`, `TpkUnityClassFlags` | TPK enums (`helpers/Tpk.py:98-135`); only the type-tree blob is used | — | L2b | — | n/a |
| `TpkDataBlob`, `TpkTypeTreeBlob`, `TpkCollectionBlob`, `TpkFileSystemBlob`, `TpkJsonBlob` | TPK blob readers (`helpers/Tpk.py:207-279`) | — | L2b | — | n/a |
| `TpkUnityClass`, `TpkClassInformation`, `TpkUnityNode`, `TpkUnityNodeBuffer`, `TpkStringBuffer`, `TpkCommonString` | TPK records (`helpers/Tpk.py:289-429`) | — | L2b | — | n/a |
| `Tpk.read_string`, `Tpk.read_data`, `Tpk.read_version`, `Tpk.read_versions` | varint-length UTF-8, i32-length bytes, u64 versions (`helpers/Tpk.py:445-469`) | — | L2b | — | n/a |
| `TypeTreeGenerator` | wraps TypeTreeGeneratorAPI; without it, construction raises `ImportError` (`helpers/TypeTreeGenerator.py:6-25`) | A | L2 | planned: provider suite | planned (P3) |
| `TypeTreeGeneratorBase` | the stand-in base when TypeTreeGeneratorAPI is missing (`helpers/TypeTreeGenerator.py:9-17`) | — | API | — | n/a |
| `TypeTreeGenerator.load_local_game` | IL2CPP (`GameAssembly.dll` + metadata) or Mono `Managed/*.dll` (`helpers/TypeTreeGenerator.py:27-39`) | A | L2 | planned: provider suite | planned (P3) |
| `TypeTreeGenerator.load_local_dll_folder` | every `.dll` in a folder (`helpers/TypeTreeGenerator.py:41-48`) | A | L2 | planned: provider suite | planned (P3) |
| `TypeTreeGenerator.get_nodes_up` | appends `.dll`, builds the tree with byte size and version 0 and the generator's meta flags, cached by `(assembly, fullname)` (`helpers/TypeTreeGenerator.py:50-75`) | A | L2 | planned: provider suite | planned (P3) |
| `TpkClassGenerator.generate_classes`, `TpkClassGenerator.generate_field_type`, `TpkClassGenerator.implement_node_class` | regenerate `classes/generated.py` from the TPK (`tools/TpkClassGenerator.py:198-376`) | — | — | — | n/a |
| `NodeClass`, `NodeClassField` | the generator's records (`tools/TpkClassGenerator.py:132-195`) | — | — | — | n/a |
| `config.FALLBACK_UNITY_VERSION` | `None`; used when a file has no usable version (`config.py:5`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `config.get_fallback_version` | raises `UnityVersionFallbackError` unless the fallback is a `str`; warns once (`config.py:21-40`) | A | API | planned: config suite | planned (P3) |
| `TypeTreeError` | carries `nodes` (`exceptions.py:1-4`); its one raise site, `files/ObjectReader.py:306`, is unreachable in 1.25.2 (section 8, no tree at all) | — | API | — | n/a |
| `UnityVersionFallbackError`, `UnityVersionFallbackWarning` | `exceptions.py:7-14` | A | API | planned: binding | planned (P3) |

## 11. Versions

Lines are in `helpers/UnityVersion.py`. `CANONICAL_DUMP.md` section 4.2 gives the packing rule.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `UnityVersion` | an `int`, `major<<48 \| minor<<32 \| build<<16 \| type<<8 \| number`; compares with ints and tuples (`:23`, `:112-142`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `UnityVersion.from_str` | the DOTALL pattern (`:7-10`), one type character, `a b c f p x` else 255, no type packs `f`; spill-over of wide fields (`:63-88`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `UnityVersion.from_list` | packs five fields (`:57-60`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `UnityVersion.major`, `UnityVersion.minor`, `UnityVersion.build`, `UnityVersion.type_number`, `UnityVersion.as_tuple` | field extraction, 16/16/16/8/8 bits (`:28-54`, `:109-110`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `UnityVersion.type` | `UnityVersionType` of the type byte; raises `ValueError` for bytes 6-254 (`:40-42`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `UnityVersion.type_str`, `UnityVersion.postfix` | the original type character and trailing text (`:44-50`) | A | API | planned: binding | planned (P3) |
| `UnityVersionType` | `a b c f p x u` = 0-5, 255 (`:13-20`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `str(UnityVersion)` | above 5.x drops the build and its dot: `2019.4.15f1` prints `2019.4f1`, and `2018.4.36` (no type) prints `2018.4f0`; 5.x and below drop the type and its number: `5.6.7f1` prints `5.6.7` (`:90-94`); section 19.2 | A | API | planned: binding | planned (P3) |

## 12. UnityCN and the decryptor

Lines are in `helpers/ArchiveStorageManager.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| encryption flag | `UsesAssetBundleEncryption` is `0x200` in `ArchiveFlagsOld` and `0x1400` in `ArchiveFlags` (`enums/BundleFile.py:12-26`); a set flag builds a decryptor (`files/BundleFile.py:123-124`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P3 |
| `ArchiveStorageManager.set_assetbundle_decrypt_key` | a 16-byte key, `str` encoded with surrogateescape; else `ValueError` (`:16-22`) | A | API | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageDecryptor` | u32 unknown, data/key vectors and signature vectors (`:62-72`); no key (`ArchiveStorageManager.DECRYPT_KEY`) raises `LookupError` (`:74-85`); the AES-ECB signature must equal `ArchiveStorageManager.UNITY3D_SIGNATURE`, `#$unity3dchina!@`, else a plain `Exception` (`:12`, `:87-89`); index and substitute tables from the decrypted key (`:91-94`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageDecryptor.decrypt_block` | UnityPyBoost's `decrypt_block` when present (`:96-98`), else the Python loop (`:100-107`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `UnityPyBoost.decrypt_block` | the C version of the loop | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageDecryptor.decrypt`, `ArchiveStorageDecryptor.decrypt_byte` | the LZ4-token-aware byte decryption (`:109-143`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageManager.decrypt_key` | AES-ECB-encrypt the key vector, XOR with data (`:33-37`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageManager.read_vector` | 16 + 16 bytes and one skipped byte (`:25-30`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageManager.brute_force_key` | tries every 16-word-character window of a file as the key (`:40-59`) | — | — | — | n/a |
| encrypted save | `save_fs` clears the encryption flag; UnityPy never writes encrypted bundles (`files/BundleFile.py:303-307`) | A | L4 | planned: save suite | planned (P3) |

## 13. Streams

Lines are in `streams/EndianBinaryReader.py` and `streams/EndianBinaryWriter.py`. They are Python I/O plumbing; their observable rules sit in the rows that use them, and these are the ones that matter.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `EndianBinaryReader` | picks a memoryview reader for bytes, a stream reader for a file object or a path, by input type; default endian `>` (`streams/EndianBinaryReader.py:33-69`). Which one an input gets decides the string rule below: `Environment` opens paths as streams (`environment.py:137`), the dumper loads bytes, and a bundle's entries are always memory | A | L0, L1 | `test_parity_rows.py` a_path_reads_strings_with_the_stream_rule | pinned P0; impl P1 |
| `EndianBinaryReader_Memoryview`, `EndianBinaryReader_Streamable`, `EndianBinaryReader_Streamable_LocalFile` | the three readers (`streams/EndianBinaryReader.py:231-332`) | — | — | — | n/a |
| `EndianBinaryReader.memory_read_func`, `EndianBinaryReader.stream_read_func` | the generated primitive readers (`streams/EndianBinaryReader.py:335-349`) | — | — | — | n/a |
| strings to NUL | surrogate-escaped UTF-8, at most 32767 bytes. A string whose NUL is in reach reads the same from both readers. With no NUL in reach, the memory reader returns 32767 bytes without consuming a NUL, or, when the window reaches the end, raises a plain `Exception` "String not terminated" (`streams/EndianBinaryReader.py:275-284`; `EndianBinaryReader.reNot0` is the search); the stream reader never raises and drops the last byte it read: 32767 bytes in for 32766 characters, or 9 characters from the last 10 bytes of a stream (`streams/EndianBinaryReader.py:136-144`) | A | L1 | `test_dump.py` l0_detection_and_name_rules (memory); `test_parity_rows.py` a_path_reads_strings_with_the_stream_rule (both) | verified P0 (L1); impl P1 |
| aligned strings | `EndianBinaryReader.read_aligned_string`: a length outside `0 < n ≤ remaining` reads `""` (`streams/EndianBinaryReader.py:266-273`). Only the pure-Python typetree reader (`byte_size == 0`) and `ShaderConverter`'s keywords (`export/ShaderConverter.py:596`, `:600`) use it. UnityPyBoost's typetree reader, the L2 path for every object with `byte_size > 0`, raises instead: a negative length is error kind `negative_length`, a length past the end `out_of_bounds` (`CANONICAL_DUMP.md` section 4.3) | A | L2 | `test_unitypy_boost.py` check_read_values (the `""` rule), error_kinds (UnityPyBoost's raises) | verified P0 (L2); impl P1 |
| reads past the end | `read` and `read_bytes` return the bytes that exist and move `Position` past the end, with no error, on both readers (`streams/EndianBinaryReader.py:249-256`); only the fixed-width primitives raise (`struct.error`). So a bundle entry or a `.resS` range that runs past its data reads short (sections 5.2), and the texture, mesh or audio code works on what it got | A | L1, L5 | `test_parity_rows.py` reads_past_the_end_return_short_data | pinned P0; impl P1 |
| `EndianBinaryWriter` | default endian `>`; `write_string_to_null` and `write_aligned_string` encode with surrogateescape (`streams/EndianBinaryWriter.py:9-130`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |

## 14. Classes

Every class in `classes/generated.py` is listed in Appendix A. They are attrs records built from the TPK by `TpkClassGenerator`; their behaviour is the typetree's (section 9). This section covers the classes UnityPy adds behaviour to, and the classes the export helpers read.

### 14.1 Base classes

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Object` | base of every generated class; fields arrive as keyword arguments; `copy.copy` rebuilds through the annotations (`classes/Object.py:11-54`) | A | API | planned: object-mode suite | planned (P3) |
| `Object.set_object_reader` | `read_typetree(wrap=True)` attaches the reader (`classes/Object.py:17-18`) | A | API | planned: object-mode suite | planned (P3) |
| `Object.assets_file` | the reader's SerializedFile, or `None` (`classes/Object.py:23-27`) | A | API | planned: object-mode suite | planned (P3) |
| `Object.save` | `object_reader.save_typetree(self)`; raises `ValueError` without a reader (`classes/Object.py:29-33`) | A | L3 | planned: object-mode resave | planned (P3) |
| `UnknownObject` | a class the generated module lacks, or one whose fields do not match; keeps the node in `__node__` (`classes/UnknownObject.py:7-14`) | A | API | planned: object-mode suite | planned (P3) |
| `UnknownObject.get_type` | the node's `m_Type`, or `None` (`classes/UnknownObject.py:16-17`) | A | API | planned: object-mode suite | planned (P3) |
| `PPtr` | `m_FileID`, `m_PathID`, `assetsfile`; truthy when the path id is not 0; equality and hash on the two ids (`classes/PPtr.py:15-19`, `:98-107`) | A | API | planned: object-mode suite | planned (P3) |
| `PPtr.file_id`, `PPtr.path_id` | aliases of the two ids (`classes/PPtr.py:21-29`) | — | API | — | n/a |
| `PPtr.type` | `deref().type` (`classes/PPtr.py:31-33`) | A | L1 (env) | planned: env dump | planned (P1) |
| `PPtr.deref` | path id 0 raises `ValueError`; file id 0 is the pointer's own file; otherwise `externals[file_id - 1]` with Python indexing, guarded only above (`classes/PPtr.py:57-60`), so a negative file id takes an external from the end (`-1` reads `externals[-2]`) or raises `IndexError`; the external's path stripped of `archive:/` and `assets/`, its last component lower-cased, matched against the parent container's entry names, else `Environment.find_file`, whose CAB lookup is the Environment's one registry (section 3); raises `FileNotFoundError`; a missing path id raises `KeyError` (`classes/PPtr.py:43-90`) | A | L1 (env) | `test_parity_rows.py` negative_file_ids_index_from_the_end; planned: env dump | pinned P0; impl P1 |
| `PPtr.deref_parse_as_object`, `PPtr.deref_parse_as_dict` | `deref()` then parse (`classes/PPtr.py:92-96`) | A | L2 | planned: env dump | planned (P1) |
| `PPtr.read`, `PPtr.read_typetree` | backward-compatible aliases of the two above (`classes/PPtr.py:35-41`) | — | API | — | n/a |
| `Vector2f`, `Vector3f`, `Vector4f` | attrs records with defaults 0 (`classes/math.py:10-37`) | A | API | planned: object-mode suite | planned (P3) |
| `float3`, `float4` | aliases of `Vector3f` and `Vector4f` (`classes/math.py:40-41`) | A | API | planned: object-mode suite | planned (P3) |
| `Quaternionf` | a `Vector4f` (`classes/math.py:44-47`) | A | API | planned: object-mode suite | planned (P3) |
| `Matrix3x4f`, `Matrix4x4f` | attrs records of named elements (`classes/math.py:50-83`) | A | API | planned: object-mode suite | planned (P3) |
| `ColorRGBA` | `r g b a` floats, or built from a packed `rgba` u32 (older trees) (`classes/math.py:86-111`) | A | API | planned: object-mode suite | planned (P3) |
| `generated.unitypy_define` | applies attrs `define` with temporarily rewritten bases (`classes/generated.py:26-44`) | — | — | — | n/a |
| `ClassIDType` | an `ExtendableEnum` of 368 class ids; an unknown id becomes `Unknown (N)` instead of raising (`enums/ClassIDType.py:5`, `enums/ExtendableEnum.py:6-19`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `ExtendableEnum` | the base that adds unknown members (`enums/ExtendableEnum.py:6-19`) | — | — | — | n/a |
| class map | `classes/ClassIDTypeToClassMap.py` maps 358 of the 368 class ids to generated classes (`classes/ClassIDTypeToClassMap.py:720`); `ObjectReader.get_class` reads it | A | API | planned: binding | planned (P3) |

### 14.2 Classes with special handling

| class | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Texture2D.image` | getter `Texture2DConverter.get_image_from_texture2d`; setter `set_image` (`classes/legacy_patch/Texture2D.py:8-11`, `:78`). The getter fetches the data before anything else (`export/Texture2DConverter.py:240-241`), so a 0×0 texture with no data raises as `Texture2D.get_image_data` does instead of returning the empty image `parse_image_data` gives (section 19.2) | A | L5 | planned: L5 goldens per format (section 17) | planned (P1) |
| `Texture2D.get_image_data` | `image_data` when non-empty; else, when `m_StreamData` is present (any `StreamingInfo` is truthy, so from 5.3 on it always is), `ResourceReader.get_resource_data` with its path, even an empty one; else `ValueError` "No image data found", which only a tree without `m_StreamData` (before 5.3) reaches (`classes/legacy_patch/Texture2D.py:63-75`). A texture with no pixel data keeps an empty stream path, which is joined to `Environment.path`: when that is a directory, `IsADirectoryError` escapes (section 5.2) | A | L5 | `test_parity_rows.py` an_empty_texture_follows_an_empty_stream_path; planned: `.resS` texture fixture | pinned P0; impl P1 |
| `Texture2D.set_image` | section 18 | | | | |
| `Texture2DArray.images` | `GraphicsFormat(m_Format)` (an unknown value raises `ValueError`) through `GraphicsFormat.GRAPHICS_TO_TEXTURE_MAP` (unmapped raises `NotImplementedError`); data from `image_data`, and from `m_StreamData` only when `image_data is None`, which a parsed array never is: a streamed array's empty TypelessData reads as `b""`, so every slice raises `ValueError` "Texture2D has no image data" (checked in the oracle on 2026-10-28; section 19.2); `m_Depth` slices of `m_DataSize // m_Depth`; decoded with platform 0 and no blob, so no Switch or Xbox handling (`classes/legacy_patch/Texture2DArray.py:12-43`, `:20-21`, `enums/GraphicsFormat.py:179-277`) | A, C | L5 | planned: array fixture | planned (P3) |
| `Sprite.image` | `SpriteHelper.get_image_from_sprite` (`classes/legacy_patch/Sprite.py:4-10`); section 15.2 | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteAtlas` | no patch: `SpriteHelper.get_image_from_sprite` dereferences `m_SpriteAtlas`, or finds an atlas in the same file whose peeked name equals `m_AtlasTags[0]`, then takes the first `m_RenderDataMap` entry whose key equals `m_RenderDataKey`; a key the atlas lacks raises `StopIteration` (`export/SpriteHelper.py:63-81`, `:78`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteRenderData` | the sprite's own render data when there is no atlas; with or without an atlas, the tight-packing mesh always comes from the sprite's own `m_RD` (`export/SpriteHelper.py:80-81`, `:117`) | A | L5 | planned: sprite goldens | planned (P1) |
| `Mesh.export` | `MeshExporter.export_mesh(self, format)` (`classes/legacy_patch/Mesh.py:4-8`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `Renderer.export` | `MeshRendererExporter.export_mesh_renderer(self, dir)`, inherited by `MeshRenderer` and `SkinnedMeshRenderer` (`classes/legacy_patch/Renderer.py:4-8`) | C | L6 | planned: divergence test | planned (P3) |
| `AudioClip.extension` | `AUDIO_TYPE_EXTEMSION.get(m_CompressionFormat, ".audioclip")`; the dict mixes `AudioType` and `AudioCompressionFormat` keys, so equal values collide and the later one wins (`classes/legacy_patch/AudioClip.py:5-6`, `enums/Audio.py:33-56`) | A | API | planned: binding | planned (P3) |
| `AudioClip.samples` | `AudioClipConverter.extract_audioclip_samples(self)` (`classes/legacy_patch/AudioClip.py:9-12`); section 16 | A, Tol | L8 | section 16 | planned (P3) |
| `Shader.export` | `ShaderConverter.export_shader(self)` (`classes/legacy_patch/Shader.py:4-7`) | A, C | L7 | planned: shader text goldens | planned (P3) |
| `GameObject.m_Components` | `[]` for `None`; `ComponentPair.component` for each pair; an empty list raises `IndexError`, and old `(classID, PPtr)` tuples raise `AttributeError` (`classes/legacy_patch/GameObject.py:9-15`); section 19.2 | A | API | planned: binding | planned (P3) |
| `GameObject.m_Animator`, `GameObject.m_Animation`, `GameObject.m_Transform`, `GameObject.m_MeshRenderer`, `GameObject.m_SkinnedMeshRenderer`, `GameObject.m_MeshFilter` | the first component whose dereferenced type matches, else `None` (`classes/legacy_patch/GameObject.py:18-33`) | A | L1 (env) | planned: binding | planned (P3) |
| `Transform` | no special handling; reached through `GameObject.m_Transform`; read through its typetree | A | L2 | corpus dump | verified P0 (L2); impl P1 |
| `Material` | no patch; `MeshRendererExporter.export_material` reads `m_SavedProperties` (`export/MeshRendererExporter.py:106-189`) | A, C | L6 | planned: MTL goldens | planned (P3) |
| `MonoBehaviour` | without an embedded tree: the TPK's head plus `Environment.typetree_generator` (`files/ObjectReader.py:300-304`); the extractor writes it as JSON (section 15.7) | A | L2, L7 | corpus dump (embedded trees) | verified P0 (L2); impl P1 |
| `MonoScript` | read to build `{namespace}.{class}` for the generator (`files/ObjectReader.py:339-346`) | A | L2 | corpus dump | verified P0 (L2); impl P1 |
| `AssetBundle` | parsed while loading for `m_Container` and `m_PreloadTable` (`files/SerializedFile.py:316-324`, `helpers/ContainerHelper.py:21-49`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `Font` | no patch; the extractor writes `m_FontData` as `.ttf`, `.otf` when it starts with `OTTO` (`tools/extractor.py:177-187`) | A | L7 | planned: font fixture | planned (P3) |
| `TextAsset` | no patch; the extractor writes `m_Script` re-encoded with surrogateescape (`tools/extractor.py:165-174`) | A | L7 | planned: text fixture | planned (P3) |
| `VideoClip` | no special handling and no exporter in UnityPy: the dict only. unityrs-export's VideoClip export goes beyond UnityPy. | A | L2 | planned: Editor fixture | planned (P3) |
| `AnimationClip`, `Avatar`, `Animator`, `AnimatorController` | no special handling: the dict only. Sampling, retargeting and GLB go beyond UnityPy (L11). | A | L2 | corpus dump | verified P0 (L2); impl P2b |

### 14.3 ContainerHelper

Lines are in `helpers/ContainerHelper.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `ContainerHelper` | from an AssetBundle (`m_Container`, `m_PreloadTable`) or a list; `container_dict` path→PPtr (last wins); `path_dict` path id→path (last wins, file id ignored) (`:12-29`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `ContainerHelper.parse_preload_table` | for each container entry with a valid preload range, dereferences each preload PPtr and sets its path in the target file's `path_dict` if absent; runs once (`:31-49`). Only `FileNotFoundError` and `KeyError` are skipped (`:43-46`): any other failure (an `IndexError` from a negative file id, an `AttributeError` from a target that is a raw reader, a loader `OSError`) escapes and fails `Environment.container` | A | L1 (env) | `test_parity_rows.py` negative_file_ids_index_from_the_end; planned: env dump | pinned P0; impl P1 |
| `ContainerHelper.items` | `(path, PPtr)` in container order, repeats kept (`:51-52`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `ContainerHelper.keys`, `ContainerHelper.values` | lists built from sets: deduplicated, in no defined order (`:54-58`) | A | API | planned: binding | planned (P3) |
| mapping protocol | `[]`, `in`, `len` (entries, repeats counted), iteration over `keys()`; attribute access by path; assignment and deletion raise `NotImplementedError` (`:60-85`) | A | API | planned: binding | planned (P3) |

## 15. Export helpers

### 15.1 Texture2DConverter

Lines are in `export/Texture2DConverter.py`. Section 17 has the per-format detail.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Texture2DConverter.get_image_from_texture2d` | `parse_image_data` with the texture's data, size, format, the reader's version (else `(0, 0, 0, 0)`) and platform, and `m_PlatformBlob` (`:237-251`) | A | L5 | planned: L5 goldens | planned (P1) |
| `Texture2DConverter.parse_image_data` | zero width or height gives an empty RGBA image (reached through `parse_image_data` itself, not through `Texture2D.image`, section 14.2); empty data raises `ValueError`; an unknown format number raises `ValueError`; then the Xbox swap, the Switch deswizzle or block padding, crunch unpacking, the decoder, a crop back to size, and a vertical flip (`:254-320`) | A | L5 | planned: L5 goldens | planned (P1) |
| output flip | `flip=True` by default: every decoded image is flipped top to bottom (`:317-318`) | A | L5 | planned: L5 goldens | planned (P1) |
| `Texture2DConverter.get_compressed_image_size` | pads to the block: ASTC by name, `DXT`/`BC`/`ETC`/`EAC` 4×4, PVRTC 8×4 (2 bpp) or 4×4; no padding for any other name, ATC included (`:22-50`) | A | L5 | planned: NPOT goldens | planned (P1) |
| `Texture2DConverter.pad_image` | pads with the last column and row (NEAREST) and the corner pixel (`:53-80`) | A | L5 | planned: `set_image` round trip | planned (P3) |
| `Texture2DConverter.swap_bytes_for_xbox` | swaps byte pairs for `Texture2DConverter.XBOX_SWAP_FORMATS` on XBOX360, first of all, so a crunched texture's crunch stream is swapped before it is unpacked (`:274-275`, `:323-328`, `:569`) | A | L5 | planned: Xbox fixture | planned (P3) |
| `Texture2DConverter.pillow` | `Image.frombytes(mode, size, data, codec, args)`, then an optional channel reorder (`:331-344`) | A | L5 | planned: L5 goldens | planned (P1) |
| `Texture2DConverter.atc` | texture2ddecoder ATC, BGRA to RGBA (`:347-353`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.astc` | astcenc into `U8`; data shorter than the block count raises `ValueError` (`:356-366`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.get_astc_context` | one LDR context per thread and block size, quality 100, `USE_DECODE_UNORM8` (`:369-387`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.calculate_astc_compressed_size` | 16 bytes per block (`:390-397`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.pvrtc` | texture2ddecoder PVRTC, 2 or 4 bpp, BGRA to RGBA (`:400-402`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.etc` | texture2ddecoder ETC1, ETC2, ETC2 A1, ETC2 A8 (`:405-417`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.eac` | texture2ddecoder EAC R, R signed, RG, RG signed (`:420-432`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.half` | `int(half * 256)` per value, truncated toward zero, `len(codec)` values per pixel, and `Texture2DConverter.CONV_TABLE` always makes `codec` the string `"raw"`, so three; a value whose result falls outside 0-255 (1.0 and above, -1/256 and below) or a NaN raises `ValueError`, an infinity `OverflowError`, and data that runs out `struct.error` (`:435-447`) | B, C | L5 | section 17 | planned (P3) |
| `Texture2DConverter.rg` | declared `(data, w, h, mode, codec, args)`, but `CONV_TABLE` passes `("RGB", "raw", <codec>)`, so `codec` is `"raw"` and `Texture2DConverter.RG_PADDING_MAP["raw"]` raises `KeyError` (`:450-468`) | B | L5 | section 17 | planned (P3) |
| `Texture2DConverter.rgb9e5float` | assigns floats into a `bytearray`, which raises `TypeError` (`:471-484`) | B | L5 | section 17 | planned (P3) |
| `Texture2DConverter.image_to_texture2d` | maps the target format to the format it writes (section 17, `set_image` column), flips, pads, compresses or packs, and swizzles for Switch (`:118-234`) | A | L5 | planned: `set_image` round trip | planned (P3) |
| `Texture2DConverter.compress_etcpak` | etcpak per format; BC7 passes `None` parameters, which etcpak 0.9.14 rejects (`:83-103`, `:95`) | A, C | L5 | planned: `set_image` round trip | planned (P3) |
| `Texture2DConverter.compress_astc` | astcenc with the cached LDR context (`:106-115`) | A | L5 | planned: `set_image` round trip | planned (P3) |

### 15.2 SpriteHelper

Lines are in `export/SpriteHelper.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `SpriteSettings` | `settingsRaw` bits: packed (0), packing mode (1), rotation (2-5), mesh type (6) (`:29-40`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpritePackingMode`, `SpritePackingRotation`, `SpriteMeshType` | the `SpriteSettings` fields (`enums/SpritePackingMode.py`, `enums/SpritePackingRotation.py`, `enums/SpriteMeshType.py`); a rotation value from 5 to 15 raises `ValueError` while the settings are read | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.get_image` | decodes the texture unflipped; with an alpha texture, merges its first band as alpha; caches in the SerializedFile's `_cache` by path id only (`:44-59`) | A, C | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.get_image_from_sprite` | atlas lookup (section 14.2); crop by `textureRect`, which Pillow rounds half to even; flips and rotations when packed; tight packing goes through the mesh; a final vertical flip (`:62-127`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.mask_sprite` | tight sprites whose UVs are all zero: a mode `1` polygon mask from the vertices, offset to the minimum and scaled by `m_PixelsToUnits`; composite for RGBA, `putalpha` otherwise (`:130-170`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.render_sprite_mesh` | tight sprites with UVs: drops the constant axis (raises for 3D), rounds positions and UVs (half to even), copies each triangle of the first submesh only, returning inside the loop; no triangles raises `ValueError` (`:173-224`, `:222`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.copy_triangle` | a triangle of the same shape is pasted through a polygon mask; any other goes through an affine transform solved by numpy (`:227-301`) | A, Tol | L5 | planned: sprite goldens, affine metric | planned (P1) |
| `SpriteHelper.linalg_solve`, `SpriteHelper.get_matrix_inverse`, `SpriteHelper.get_matrix_determinant`, `SpriteHelper.get_matrix_minor`, `SpriteHelper.transpose_matrix` | the solver used without numpy (`:304-346`); the oracle has numpy | — | — | — | n/a |

### 15.3 MeshHelper and PackedBitVector

Lines are in `helpers/MeshHelper.py` unless named.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `MeshHandler` | the version from the argument or the Mesh's reader (a SpriteRenderData needs one); endianness defaults to `<` and no caller passes another (`:71-108`) | A, C | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.process` | channels and streams by version (below 4, 4, 5 and up); `m_StreamData` through `ResourceReader`; 16- or 32-bit indices; vertex data from 3.5; compressed mesh from 2.6 (`:110-195`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.copy_from_mesh` | fills unset arrays from the Mesh's own vertex, normal, tangent, UV, colour and skin arrays (`:197-233`) | A | L6 | planned: mesh numbers | planned (P2a) |
| `MeshHandler.copy_from_spriterenderdata` | index buffer, vertices and UVs from the sprite's render data (`:235-251`) | A | L6 | planned: sprite goldens | planned (P1) |
| `MeshHandler.get_streams` | derived streams, offsets aligned to 16 (`:256-281`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.get_channels` | the pre-4 fixed channel layout (`:283-321`) | A | L6 | planned: Editor fixture | planned (P4) |
| `MeshHandler.read_vertex_data` | per channel through `UnityPyBoost.unpack_vertexdata`; pre-2018 colour forced to 4 components (`:323-393`) | A, C | L6 | planned: mesh numbers | planned (P1) |
| `UnityPyBoost.unpack_vertexdata` | the C strided gather, with byte swapping (`:366-376`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.assign_channel_vertex_data` | channel index to attribute, for 2018 and later and for earlier versions; an unknown channel raises (`:395-449`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.get_channel_dtype` | `VertexChannelFormat` below 2017, `VertexFormat2017` below 2019, `VertexFormat` after (`:451-462`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.get_channel_component_size` | struct size of that type (`:464-466`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.decompress_compressed_mesh` | vertices, UVs, bind poses (below 5), normals, tangents, float colours (5 and up), skin, triangles, packed colours (`:468-628`) | A, C | L6 | planned: mesh numbers | planned (P2a) |
| `MeshHandler.get_triangles` | per submesh: triangles; strips (below 4, or strip topology) skipping degenerates and alternating winding, and rewriting `indexCount`; quads as two triangles; lines and points raise `ValueError` (`:630-683`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHelper.vector_list_to_tuples`, `MeshHelper.lists_to_tuples`, `MeshHelper.zeros` | list helpers (`:38-60`) | — | — | — | n/a |
| `MeshHelper.normalize` | unit vector, zeros below length 1e-5 (`:63-68`) | A | L6 | planned: mesh numbers | planned (P2a) |
| `PackedBitVector.unpack_ints` | little-endian bit stream, `m_BitSize` bits per item from `start` (`helpers/PackedBitVector.py:20-65`) | A | L6 | planned: mesh numbers | planned (P1) |
| `PackedBitVector.unpack_floats` | bit size 0 repeats `m_Start`; else `x * range / (2^bits - 1) + start` in f64 (`helpers/PackedBitVector.py:68-85`) | A | L6 | planned: mesh numbers | planned (P1) |
| `PackedBitVector.reshape` | rows of 1D or 2D shape (`helpers/PackedBitVector.py:7-17`) | — | — | — | n/a |
| `VertexChannelFormat`, `VertexFormat2017`, `VertexFormat` | struct codes per format in `VertexFormat.VERTEX_CHANNEL_FORMAT_STRUCT_TYPE_MAP`, `VertexFormat.VERTEX_FORMAT_2017_STRUCT_TYPE_MAP` and `VertexFormat.VERTEX_FORMAT_STRUCT_TYPE_MAP`; UNorm, SNorm and the byte colour formats read as raw integers (`enums/VertexFormat.py:4-80`) | A, C | L6 | planned: mesh numbers | planned (P1) |
| `MeshTopology`, `GfxPrimitiveType` | topology enums (`enums/MeshTopology.py`, `enums/GfxPrimitiveType.py`) | A | L6 | planned: mesh numbers | planned (P1) |

### 15.4 MeshExporter and MeshRendererExporter

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `MeshExporter.export_mesh` | `"obj"` only, else `NotImplementedError` (`export/MeshExporter.py:11-14`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `MeshExporter.export_mesh_obj` | `g name`, optional `mtllib`; `v` with x negated, `vt`, and `vn` with x negated, every number as `{:.9G}`: upper-case `1E-10`, `-0` for a negated zero, and `NAN`/`INF`, because the `.replace("nan", "0")` meant for NaN never matches the upper-case text (checked in the oracle on 2026-10-28; section 19.2); per submesh `g name_i`, optional `usemtl`, faces `c/c/c b/b/b a/a/a` 1-based; returns `False` without vertices (`export/MeshExporter.py:17-56`, `:33`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `MeshRendererExporter.get_mesh` | `SkinnedMeshRenderer.m_Mesh`, or the GameObject's MeshFilter mesh (`export/MeshRendererExporter.py:25-45`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `MeshRendererExporter.export_mesh_renderer` | materials per submesh offset by `m_StaticBatchInfo.firstSubMesh` or `min(m_SubsetIndices)`; writes each material texture as `{name or key}.png`, the `.obj` and the `.mtl` (`export/MeshRendererExporter.py:48-103`) | C | L6 | planned: divergence test | planned (P3) |
| `MeshRendererExporter.export_material` | `newmtl`; `Ka`/`Kd`/`Ks` from `_SColor`/`_Color`/`_SpecularColor` with defaults, `Tr`, `Ns`, all `{:.4f}`; texture maps by key (`export/MeshRendererExporter.py:106-189`). Only the defaults ever print: a material holding any of the three colours raises `AttributeError`, because `clt` reads `color.R` and `ColorRGBA` has `r g b a` (`export/MeshRendererExporter.py:110`; `classes/math.py:86-91`), and one holding any texture raises `ValueError` (`export/MeshRendererExporter.py:168`); both checked in the oracle on 2026-10-28 | A, C | L6 | planned: MTL goldens | planned (P3) |

### 15.5 ShaderConverter

Lines are in `export/ShaderConverter.py`. `export/__init__.py` does not import it; `Shader.export` does.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `ShaderConverter.export_shader` | 5.3-5.4 `m_SubProgramBlob` through LZ4, read big-endian (the reader's default), with `GpuProgramIndex N` substituted; 5.5 and up `compressedBlob`, read little-endian (`:84`); else `m_Script` as UTF-8; every result starts with `ShaderConverter.HEADER`, the `NOTE: This is *not* a valid shader file` banner (`:30-55`) | A, C | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShader` | one LZ4 slice per platform, nested-list entries take their first item, stops at the shorter length list (`:58-89`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShaderParsedForm` | `Shader`, properties, subshaders, fallback, custom editor (`:92-113`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedSubShader` | LOD, tags, passes (`:116-130`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedPass` | Pass, UsePass, GrabPass; state and the five program kinds (`:133-181`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedSubPrograms` | groups by blob index and program type; the first usable platform per group (`:184-227`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShaderState` | name, LOD, tags, cull, ZTest, ZWrite, colour mask, blend, stencil, fog (`:230-292`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShaderRTBlendState` | an empty string; its caller is commented out (`:295-298`) | — | L7 | — | n/a |
| `ShaderConverter.ConvertSerializedTagMap` | `Tags { "k" = "v" }` (`:301-311`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedProperties`, `ShaderConverter.ConvertSerializedProperty` | property block; a property type other than Color, Vector, Float, Range and Texture raises `ValueError`, Int (5, from Unity 2021.1) included (`:314-372`, `:369`; the enum stops at 4, `enums/SerializedPropertyType.py:4-9`) | A, C | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.CheckGpuProgramUsable` | platform/program-type table; NaCl, Flash and PSM raise `NotImplementedError`, and so does every platform the table lacks: PS3, `kShaderCompPlatformNone` and anything above 20 (`:375-493`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.GetPlatformString` | platform names, `unknown` otherwise (`:496-540`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderProgram` | sub-program table, entry size 12 from 2019.3 and 8 before (`:543-568`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderSubProgram` | sub-program versions 201509030 to 202012090: keywords, local keywords in `[201806140, 202012090)`, code; GLSL, Metal and console code as text, DX9 and DX11 as a `// shader disassembly not supported on DXBC` comment, SPIR-V as nothing, any other type as a `//shader disassembly not supported on {type}` comment (`:571-689`); an unknown program type raises `ValueError` (`:589`) | A, C | L7 | planned: shader text goldens; divergence test | planned (P3) |
| `PassType`, `SerializedPropertyType`, `ShaderCompilerPlatform`, `ShaderGpuProgramType`, `TextureDimension` | enums the converter reads (`enums/PassType.py`, `enums/SerializedPropertyType.py`, `enums/ShaderCompilerPlatform.py`, `enums/ShaderGpuProgramType.py`, `enums/TextureDimension.py`); an unknown program type raises `ValueError` (`:589`) | A | L7 | planned: shader text goldens | planned (P3) |

### 15.6 TextureSwizzler

Lines are in `helpers/TextureSwizzler.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TextureSwizzler.is_switch_swizzled` | Switch, a platform blob of at least 12 bytes, and more than one GOB per block (`:121-127`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.get_switch_gobs_per_block` | `1 << u32le(blob[8:12])` (`:117-118`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.deswizzle`, `TextureSwizzler.swizzle` | GOB walk over 16-byte texels (`:17-69`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.get_padded_texture_size` | pads to whole GOB blocks (`:106-114`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.ceil_divide` | integer ceiling (`:13-14`) | — | — | — | n/a |
| Switch format table | 29 formats with a texel block; any other format raises `KeyError` when swizzled (`:73-103`) | A | L5 | planned: Switch fixture | planned (P3) |

### 15.7 The extractor

Lines are in `tools/extractor.py`. Level L9 is its output tree.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `extractor.extract_assets` | loads the source; by container (default) in `extractor.EXPORT_TYPES` priority (a stable sort), paths split on `/` minus `ignore_first_container_dirs`, every entry exported with no de-duplication, so a later entry with the same path overwrites an earlier one (a Texture2D its Sprite's PNG); or every object with its name appended, skipping what an earlier export returned. Nothing catches a failure: the first export that raises ends the run (`:81-157`). | A, C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.export_obj` | export function by type (unknown types through `exportMonoBehaviour` when asked); the filter sees the parsed object; the path's own extension (the container's, or one in the object's name) is split off and handed to the export function, which keeps it; `append_path_id` appends `_None`, not the path id, because it reads `m_PathID` from the `ObjectReader`, whose attribute is `path_id` (`:25-78`, `:75`; `files/ObjectReader.py:223-225`); checked in the oracle on UnityPy's samples on 2026-10-28 | A, C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.exportTextAsset` | `{fp}.txt` (or the container's extension) from `m_Script` re-encoded with surrogateescape (`:165-174`) | A | L7 | planned: text fixture | planned (P3) |
| `extractor.exportFont` | `m_FontData` as `.ttf`, or `.otf` for `OTTO`; nothing without data (`:177-187`) | A | L7 | planned: font fixture | planned (P3) |
| `extractor.exportMesh` | `obj.export()` as UTF-8 text to `{fp}.obj`, or to the container's extension (`.asset` in UnityPy's samples), so a vertex-less mesh writes `False` and raises `TypeError` (`:190-197`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `extractor.exportShader` | `obj.export()` as UTF-8 text (`:200-207`) | A | L7 | planned: shader text goldens | planned (P3) |
| `extractor.exportMonoBehaviour` | `json.dumps(parse_as_dict(), indent=4, ensure_ascii=False)` encoded with surrogateescape, else the raw bytes as `.bin` (`:210-224`). Through `export_obj` it never returns: for a MonoBehaviour it writes the JSON and then raises `AttributeError` on `obj.path_id` (`:224`), and for any other type (`export_unknown_as_typetree`) the parsed object has no `parse_as_dict` or `get_raw_data`, so it raises `AttributeError` before writing; checked in the oracle on 2026-10-28 | A, C (proposed, section 19.2) | L7 | planned: MonoBehaviour JSON goldens | planned (P3) |
| `extractor.exportAudioClip` | one sample written as `{fp}.wav` whatever its format; several into a folder `{fp}/` as `{name}.wav`, where each name already ends in `.wav`, so `x.wav.wav` and `x-1.wav.wav` (`:227-247`) | A | L9 | planned: extractor tree | planned (P3) |
| `extractor.exportSprite` | `sprite.image` saved as `{fp}{extension}`, Pillow choosing the format by extension, with no default: an extension-less path (every sprite in the no-container mode) raises `ValueError` "unknown file extension" (checked in the oracle on UnityPy's samples on 2026-10-28). Returns the sprite and its texture, keyed by the pointer's own file; the alpha texture never, because it reads `assets_file` from a `PPtr`, whose attribute is `assetsfile` (`:250-268`, `:264-267`) | A, C (proposed, section 19.2) | L5b | planned: PNG bytes | planned (P1) |
| `extractor.exportTexture2D` | `obj.image` saved when `m_Width` is non-zero, as `{fp}.png` without an extension, else with the container's extension, which picks Pillow's format: `.tga` writes a TGA, `.psd` raises `KeyError`, `.jpg` raises `OSError` for RGBA (checked against Pillow 9.5.0 on 2026-10-28) (`:271-281`) | A | L5b | planned: PNG bytes | planned (P1) |
| PNG bytes | Pillow 9.5.0 `Image.save` defaults, for the paths that end in `.png` or get it by default: the encoder `unityrs-imageio` matches (`DESIGN.md` section 4) | A | L5b | planned: PNG bytes | planned (P1) |
| `extractor.exportGameObject` | meant to crawl every reachable PPtr and export each non-GameObject into a folder, printing failures (`:284-304`), but it always raises `AttributeError`: `crawl_obj` reads `m_PathID` from the parsed GameObject, which has none (`:350`). In container mode GameObjects sort first, so a container holding one exports nothing. Checked in the oracle on 2026-10-28. | C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.crawl_obj`, `extractor.flatten` | the crawl: PPtrs by path id alone, nested lists, tuples and dicts flattened (`:327-368`); a parsed object raises at `obj.m_PathID` (`:350`), and a null pointer would raise at `obj.m_Index`, which `PPtr` lacks (`:336`) | C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.MONOBEHAVIOUR_TYPETREES` | an empty dict nothing reads (`:324`) | — | — | — | n/a |

## 16. Audio

`DESIGN.md` section 3 fixes the format list: Ogg, RIFF and m4a pass through; FSB5 PCM, Vorbis, ADPCM and FADPCM decode in pure Rust; MPEG and Opus are optional; anything else is reported unsupported. FMOD is never bundled (`DESIGN.md` section 2). Lines are in `export/AudioClipConverter.py`; fmod_toolkit is 0.1.3. **gap** in the tier column marks a deliberate `DESIGN.md` gap: UnityPy produces output through FMOD, and unityrs reports the format unsupported.

| format | UnityPy 1.25.2 | unityrs | tier | level | test | status |
|---|---|---|---|---|---|---|
| data source | `m_AudioData` (old clips), else `m_Resource` through `ResourceReader`; neither raises `ValueError` (`:21-33`) | same | A | L8 | planned: audio fixtures | planned (P3) |
| Ogg (`OggS`) | returned unchanged as `{name}.ogg` (`:35-37`) | passthrough | A | L8 | planned: audio fixtures | planned (P3) |
| RIFF | returned unchanged as `{name}.wav` (`:38-39`) | passthrough | A | L8 | planned: audio fixtures | planned (P3) |
| m4a (`ftyp` at offset 4) | returned unchanged as `{name}.m4a` (`:40-41`) | passthrough | A | L8 | planned: audio fixtures | planned (P3) |
| anything else | `fmod_toolkit.raw_to_wav` with `m_Channels or 2` and `m_Frequency or 44100`; one WAV per subsound, `{name}.wav` or `{name}-{i}.wav` (`:43-49`) | FSB5 parser | A | L8 | planned: audio fixtures | planned (P3) |
| FSB5 PCM8, PCM16, PCM24, PCM32 | FMOD's samples in a 44-byte RIFF header, as they are | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 PCM float | FMOD's floats converted to int16 with numpy, `(v * 32768).clip(-32768, 32767)` truncated toward zero (fmod_toolkit 0.1.3, `fmod.py` lines 195-198) | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 Vorbis | FMOD decode to PCM | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 IMA ADPCM | FMOD decode to PCM | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 FADPCM | FMOD decode to PCM | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 MPEG | FMOD decode to PCM | optional feature | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 Opus | FMOD decode to PCM | optional feature | Tol | L8 | planned: against FMOD | planned (P3) |
| any other codec (XMA, ATRAC9, VAG, HEVAG, GC ADPCM, …) | whatever the bundled `libfmod` decodes; a non-PCM result raises `TypeError`, not the `NotImplementedError` fmod_toolkit means to raise, because its message concatenates a `str` and a `SOUND_FORMAT` enum (fmod_toolkit 0.1.3, `fmod.py` line 139; checked in the oracle on 2026-10-28) | reported unsupported | gap | L8 | planned: error record | planned (P3) |
| tracker and legacy audio (IT, MOD, S3M, XM, AIFF, pre-5 MP3) | FMOD | reported unsupported | gap | L8 | planned: error record | planned (P3) |
| `AudioClipConverter.extract_audioclip_samples` | the dispatcher above (`:13-49`) | same | A | L8 | planned: audio fixtures | planned (P3) |
| `AudioType`, `AudioCompressionFormat` | enums (`enums/Audio.py:4-31`) | same | A | API | planned: binding | planned (P3) |
| `Audio.AUDIO_TYPE_EXTEMSION` | extension table (`enums/Audio.py:33-56`); see `AudioClip.extension` | same | A | API | planned: binding | planned (P3) |

## 17. TextureFormat matrix

One row per `TextureFormat` value (78; 37-40 are unassigned). Lines are in `export/Texture2DConverter.py` unless named; the conversion table is `Texture2DConverter.CONV_TABLE` (`:487-566`).

**Columns.**
- **decoder** and **conversion**: what `parse_image_data` runs and how the bytes become pixels. Pillow's `bcn` is `BcnDecode.c` in Pillow 9.5.0; texture2ddecoder (1.0.5) returns BGRA, which Pillow's `BGRA` raw mode turns into RGBA; ASTC goes through astcenc (astc-encoder-py 0.1.12) with the LDR profile and `USE_DECODE_UNORM8` (`:369-387`). The pixel values quoted were checked in the oracle on 2026-10-28.
- **mode**: the Pillow mode UnityPy returns. Every image is then flipped top to bottom (`:317-318`).
- **gates**: `pad W×H` is the block padding before decoding and the crop after (`Texture2DConverter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE`, `:22-50`, `:314-315`); `no pad` means none. `sw W×H` is the Switch texel block (`TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP`, `helpers/TextureSwizzler.py:73-103`), used when `TextureSwizzler.is_switch_swizzled` holds, in place of the block padding; `sw —` means a swizzled texture of that format raises `KeyError`. `xbox` swaps byte pairs on XBOX360, before anything else, crunch unpacking included (`:274-275`, `:569`). `crunch` unpacks with `unpack_unity_crunch` from 2017.3 and for the two ETC crunched formats at any version, and with `unpack_crunch` before 2017.3 (`:297-306`).
- **UnityPy 1.25.2**: the result of `parse_image_data` on an 8×8 texture, StandaloneWindows64, version 2021.3.1f3. `raises` names the exception, module-qualified when it is not a builtin, on `0x30` bytes, as many as a real 8×8 texture of the format holds. `works`, `wrong` and `unverified` return an image in the listed mode, checked down to the pixels on input built from the format's layout (raw formats from a byte pattern, block formats from one solid-colour block per block position): `works` gives the pixels that layout fixes and the conversion column states; `wrong` gives the wrong pixels the conversion column states, not those (section 19); `unverified` has no reference to check against, and the pixels UnityPy returns are pinned. `crunch` rows need valid crunch data and were not decoded.
- **`set_image` writes**: the format `image_to_texture2d` writes for that target from an RGBA image (`:118-234`).
- **tier** and **status**: section 0. Tier A rows get L5 goldens against the oracle, Tier B rows an independent reference, Tier C rows a pinned divergence test; `DESIGN.md` section 5 puts BC7, DXT1, DXT5, RGBA32 and RGB24 in P1 and every other format in P3.

`test_texture_matrix.py` checks the mode, UnityPy and `set_image` columns of every row against the oracle (texture_matrix_matches_the_oracle), and the pixels of every row that returns an image (pixels_match_the_matrix).

| value | format | decoder | mode | conversion | gates | UnityPy 1.25.2 | `set_image` writes | tier | status |
|---|---|---|---|---|---|---|---|---|---|
| 1 | `Alpha8` | Pillow raw `A` | RGBA | (0, 0, 0, A) | no pad; sw 16×1 | works (`:489`) | `Alpha8` | A | planned (P3) |
| 2 | `ARGB4444` | Pillow raw `RGBA;4B` | RGBA | bands reordered (2, 1, 0, 3) | no pad; sw 8×1 | works (`:490`) | `RGBA32` | A | planned (P3) |
| 3 | `RGB24` | Pillow raw `RGB` | RGB | as stored | no pad; swizzled data is read as `RGBA32`, so mode RGBA (`:279-283`) | works (`:491`) | `RGB24` | A | planned (P1) |
| 4 | `RGBA32` | Pillow raw `RGBA` | RGBA | as stored | no pad; sw 4×1 | works (`:492`) | `RGBA32` | A | planned (P1) |
| 5 | `ARGB32` | Pillow raw `ARGB` | RGBA | alpha moved last | no pad; sw 4×1 | works (`:493`) | `RGBA32` | A | planned (P3) |
| 6 | `ARGBFloat` | Pillow raw `RGBAF` | — | — | no pad; sw 1×1 | raises `ValueError`, unknown raw mode (`:494`) | `RGBA32` | B | planned (P3) |
| 7 | `RGB565` | Pillow raw `BGR;16` | RGB | red from the top 5 bits; each field widened as `v * 255 / 31` (green `/ 63`), truncated: red 16 gives 131, where bit replication gives 132 | no pad; sw 8×1; xbox | works (`:495`) | `RGB24` | A | planned (P3) |
| 8 | `BGR24` | Pillow raw `BGR` | RGB | blue and red swapped | no pad; swizzled data is read as `BGRA32`, so mode RGBA (`:279-283`) | works (`:496`) | `RGB24` | A | planned (P3) |
| 9 | `R16` | Pillow raw `R;16` into RGB | — | — | no pad; sw 8×1 | raises `ValueError`, unknown raw mode (`:498`) | `R8` | B | planned (P3) |
| 10 | `DXT1` | Pillow `bcn` 1 | RGBA | 1-bit alpha | pad 4×4; sw 8×4; xbox | works (`:500`) | `DXT1` | A | planned (P1) |
| 11 | `DXT3` | Pillow `bcn` 2 | RGBA | explicit 4-bit alpha | pad 4×4; sw — | works (`:501`) | `RGBA32` | A | planned (P3) |
| 12 | `DXT5` | Pillow `bcn` 3 | RGBA | interpolated alpha | pad 4×4; sw 4×4; xbox | works (`:502`) | `DXT5` | A | planned (P1) |
| 13 | `RGBA4444` | Pillow raw `RGBA;4B` | RGBA | bands reordered (3, 2, 1, 0) | no pad; sw 8×1 | works (`:503`) | `RGBA32` | A | planned (P3) |
| 14 | `BGRA32` | Pillow raw `BGRA` | RGBA | blue and red swapped | no pad; sw 4×1 | works (`:504`) | `RGBA32` | A | planned (P3) |
| 15 | `RHalf` | `half` into Pillow mode `R` | — | — | no pad; sw — | raises `struct.error`: `half` reads three halves a pixel for the one stored, so real-sized data runs out; a value outside [0, 1) raises `ValueError` first, and Pillow's missing mode `R` (`ValueError` too) is reached only with three times the data (`:505`, `:446`) | `R8` | B | planned (P3) |
| 16 | `RGHalf` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:506`, `:459-468`) | `RGBA32` | B | planned (P3) |
| 17 | `RGBAHalf` | `half` into Pillow `RGB` | RGB | three halves per pixel for four stored, each `int(h * 256)`: alpha dropped, pixels misaligned (`(.5, .25, .125, .75)` repeated gives `(128, 64, 32)`, `(192, 128, 64)`, `(32, 192, 128)`, …) | no pad; sw — | wrong; a value at 1.0 or above, at -1/256 or below, or NaN raises `ValueError`, an infinity `OverflowError` (`:507`, `:435-447`) | `RGBA32` | C | planned (P3) |
| 18 | `RFloat` | Pillow raw `RF` into RGB | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:508`) | `R8` | B | planned (P3) |
| 19 | `RGFloat` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:509`) | `RGBA32` | B | planned (P3) |
| 20 | `RGBAFloat` | Pillow raw `RGBAF` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:510`) | `RGBA32` | B | planned (P3) |
| 21 | `YUY2` | none: the entry is commented out (`:511`) | — | — | no pad; sw — | raises `NotImplementedError` (`:308-309`) | `RGBA32` | B | planned (P3) |
| 22 | `RGB9e5Float` | `rgb9e5float` | — | — | no pad; sw — | raises `TypeError`, floats into a bytearray (`:512`, `:482`) | `RGB24` | B | planned (P3) |
| 23 | `RGBFloat` | none: no entry | — | — | no pad; sw — | raises `NotImplementedError` (`:308-309`) | `RGBA32` | B | planned (P3) |
| 24 | `BC6H` | Pillow `bcn` 6 | — | — | pad 4×4; sw 4×4 | raises `ValueError`, bad image mode (`:515`) | `RGBA32` | B | planned (P3) |
| 25 | `BC7` | Pillow `bcn` 7 | RGBA | as decoded | pad 4×4; sw 4×4 | works (`:516`) | raises `ValueError` (`:95`); section 19.2 | A | planned (P1) |
| 26 | `BC4` | Pillow `bcn` 4 | L | one channel | pad 4×4; sw 8×4 | works (`:513`) | `BC4` | A | planned (P3) |
| 27 | `BC5` | Pillow `bcn` 5 | RGB | (R, G, 0) | pad 4×4; sw 4×4 | works (`:514`) | `BC5` | A | planned (P3) |
| 28 | `DXT1Crunched` | crunch, then Pillow `bcn` 1 | RGBA | 1-bit alpha | pad 4×4; sw —; xbox; crunch | crunch (`:517`) | `DXT1` | A | planned (P3) |
| 29 | `DXT5Crunched` | crunch, then Pillow `bcn` 3 | RGBA | interpolated alpha | pad 4×4; sw —; xbox; crunch | crunch (`:518`) | `DXT5` | A | planned (P3) |
| 30 | `PVRTC_RGB2` | texture2ddecoder PVRTC 2 bpp | RGBA | BGRA to RGBA | pad 8×4; sw — | works (`:519`) | `RGB24` | A | planned (P3) |
| 31 | `PVRTC_RGBA2` | texture2ddecoder PVRTC 2 bpp | RGBA | BGRA to RGBA | pad 8×4; sw — | works (`:520`) | `RGBA32` | A | planned (P3) |
| 32 | `PVRTC_RGB4` | texture2ddecoder PVRTC 4 bpp | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:521`) | `RGB24` | A | planned (P3) |
| 33 | `PVRTC_RGBA4` | texture2ddecoder PVRTC 4 bpp | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:522`) | `RGBA32` | A | planned (P3) |
| 34 | `ETC_RGB4` | texture2ddecoder ETC1 | RGBA | alpha 255 | pad 4×4; sw — | works (`:523`) | `ETC_RGB4` | A | planned (P3) |
| 35 | `ATC_RGB4` | texture2ddecoder ATC RGB | RGBA | alpha 255 | no pad; sw — | works (`:524`) | `RGB24` | A | planned (P3) |
| 36 | `ATC_RGBA8` | texture2ddecoder ATC RGBA | RGBA | BGRA to RGBA | no pad; sw — | works (`:525`) | `RGBA32` | A | planned (P3) |
| 41 | `EAC_R` | texture2ddecoder EAC R | RGBA | (R, 0, 0, 255) | pad 4×4; sw — | works (`:526`) | `R8` | A | planned (P3) |
| 42 | `EAC_R_SIGNED` | texture2ddecoder EAC R signed | RGBA | (R + 127, 0, 0, 255), -128 clamped to -127: base 100 gives 227, -100 gives 27 | pad 4×4; sw — | works (`:527`) | `R8` | A | planned (P3) |
| 43 | `EAC_RG` | texture2ddecoder EAC RG | RGBA | (R, G, 0, 255) | pad 4×4; sw — | works (`:528`) | `RGBA32` | A | planned (P3) |
| 44 | `EAC_RG_SIGNED` | texture2ddecoder EAC RG signed | RGBA | (R + 127, G + 127, 0, 255) | pad 4×4; sw — | works (`:529`) | `RGBA32` | A | planned (P3) |
| 45 | `ETC2_RGB` | texture2ddecoder ETC2 | RGBA | alpha 255 | pad 4×4; sw — | works (`:530`) | `ETC2_RGB` | A | planned (P3) |
| 46 | `ETC2_RGBA1` | texture2ddecoder ETC2 A1 | RGBA | BGRA to RGBA; a transparent punch-through pixel keeps its colour and gets alpha 0 | pad 4×4; sw — | wrong: the Khronos Data Format Specification 1.3, section 22.9, makes a transparent punch-through pixel (0, 0, 0, 0) (`:531`) | `ETC2_RGBA8` | C | planned (P3) |
| 47 | `ETC2_RGBA8` | texture2ddecoder ETC2 A8 | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:532`) | `ETC2_RGBA8` | A | planned (P3) |
| 48 | `ASTC_RGB_4x4` | astcenc LDR | RGBA | as decoded | pad 4×4; sw 4×4 | works (`:533`) | `ASTC_RGB_4x4` | A | planned (P3) |
| 49 | `ASTC_RGB_5x5` | astcenc LDR | RGBA | as decoded | pad 5×5; sw 5×5 | works (`:534`) | `ASTC_RGB_5x5` | A | planned (P3) |
| 50 | `ASTC_RGB_6x6` | astcenc LDR | RGBA | as decoded | pad 6×6; sw 6×6 | works (`:535`) | `ASTC_RGB_6x6` | A | planned (P3) |
| 51 | `ASTC_RGB_8x8` | astcenc LDR | RGBA | as decoded | pad 8×8; sw 8×8 | works (`:536`) | `ASTC_RGB_8x8` | A | planned (P3) |
| 52 | `ASTC_RGB_10x10` | astcenc LDR | RGBA | as decoded | pad 10×10; sw 10×10 | works (`:537`) | `ASTC_RGB_10x10` | A | planned (P3) |
| 53 | `ASTC_RGB_12x12` | astcenc LDR | RGBA | as decoded | pad 12×12; sw 12×12 | works (`:538`) | `ASTC_RGB_12x12` | A | planned (P3) |
| 54 | `ASTC_RGBA_4x4` | astcenc LDR | RGBA | as decoded | pad 4×4; sw 4×4 | works (`:539`) | `ASTC_RGBA_4x4` | A | planned (P3) |
| 55 | `ASTC_RGBA_5x5` | astcenc LDR | RGBA | as decoded | pad 5×5; sw 5×5 | works (`:540`) | `ASTC_RGBA_5x5` | A | planned (P3) |
| 56 | `ASTC_RGBA_6x6` | astcenc LDR | RGBA | as decoded | pad 6×6; sw 6×6 | works (`:541`) | `ASTC_RGBA_6x6` | A | planned (P3) |
| 57 | `ASTC_RGBA_8x8` | astcenc LDR | RGBA | as decoded | pad 8×8; sw 8×8 | works (`:542`) | `ASTC_RGBA_8x8` | A | planned (P3) |
| 58 | `ASTC_RGBA_10x10` | astcenc LDR | RGBA | as decoded | pad 10×10; sw 10×10 | works (`:543`) | `ASTC_RGBA_10x10` | A | planned (P3) |
| 59 | `ASTC_RGBA_12x12` | astcenc LDR | RGBA | as decoded | pad 12×12; sw 12×12 | works (`:544`) | `ASTC_RGBA_12x12` | A | planned (P3) |
| 60 | `ETC_RGB4_3DS` | texture2ddecoder ETC1 | RGBA | plain ETC1 blocks in plain order; any 3DS tiling or byte order is ignored | pad 4×4; sw — | unverified (`:545`) | `ETC_RGB4_3DS` holding plain ETC1 | U | planned (P3) |
| 61 | `ETC_RGBA8_3DS` | texture2ddecoder ETC1 | RGBA | the first half of the 16-byte-per-block data read as ETC1 blocks, alpha 255; the alpha is never decoded | pad 4×4; sw — | unverified (`:546`) | `RGBA32` | U | planned (P3) |
| 62 | `RG16` | `rg` | — | — | no pad; sw 8×1 | raises `KeyError` `'raw'` (`:499`) | `RGBA32` | B | planned (P3) |
| 63 | `R8` | Pillow raw `R` into RGB | RGB | (R, 0, 0) | no pad; sw 16×1 | works (`:497`) | `R8` | A | planned (P3) |
| 64 | `ETC_RGB4Crunched` | crunch, then texture2ddecoder ETC1 | RGBA | alpha 255 | pad 4×4; sw —; crunch | crunch (`:547`) | `ETC_RGB4Crunched` holding plain ETC1; section 19.2 | A | planned (P3) |
| 65 | `ETC2_RGBA8Crunched` | crunch, then texture2ddecoder ETC2 A8 | RGBA | BGRA to RGBA | pad 4×4; sw —; crunch | crunch (`:548`) | `ETC2_RGBA8` | A | planned (P3) |
| 66 | `ASTC_HDR_4x4` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile: an FP16 constant-colour block decodes as white, because astcenc's LDR profile writes 0xFFFF for one | pad 4×4; sw — | wrong (`:549`, `:371-377`) | `ASTC_RGBA_4x4` (`ASTC_RGB_4x4` from an RGB image) | C | planned (P3) |
| 67 | `ASTC_HDR_5x5` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile, as `ASTC_HDR_4x4` | pad 5×5; sw — | wrong (`:550`) | `ASTC_RGBA_5x5` (`ASTC_RGB_5x5` from an RGB image) | C | planned (P3) |
| 68 | `ASTC_HDR_6x6` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile, as `ASTC_HDR_4x4` | pad 6×6; sw — | wrong (`:551`) | `ASTC_RGBA_6x6` (`ASTC_RGB_6x6` from an RGB image) | C | planned (P3) |
| 69 | `ASTC_HDR_8x8` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile, as `ASTC_HDR_4x4` | pad 8×8; sw — | wrong (`:552`) | `ASTC_RGBA_8x8` (`ASTC_RGB_8x8` from an RGB image) | C | planned (P3) |
| 70 | `ASTC_HDR_10x10` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile, as `ASTC_HDR_4x4` | pad 10×10; sw — | wrong (`:553`) | `ASTC_RGBA_10x10` (`ASTC_RGB_10x10` from an RGB image) | C | planned (P3) |
| 71 | `ASTC_HDR_12x12` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile, as `ASTC_HDR_4x4` | pad 12×12; sw — | wrong (`:554`) | `ASTC_RGBA_12x12` (`ASTC_RGB_12x12` from an RGB image) | C | planned (P3) |
| 72 | `RG32` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:555`) | `RGBA32` | B | planned (P3) |
| 73 | `RGB48` | Pillow raw `RGB;16` | RGB | Pillow reads `RGB;16` as packed 5-6-5 (red in the low five bits), two bytes per pixel for six stored: pure red `(0xFFFF, 0, 0)` gives white, black, black, white, … in data order | no pad; sw — | wrong (`:556`) | `RGBA32` | C | planned (P3) |
| 74 | `RGBA64` | Pillow raw `RGBA;16` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:557`) | `RGBA32` | B | planned (P3) |
| 75 | `R8_SIGNED` | Pillow raw `R;8s` into mode `R` | — | — | no pad; sw — | raises `ValueError`, unrecognized image mode (`:558`) | `RGBA32` | B | planned (P3) |
| 76 | `RG16_SIGNED` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:559`) | `RGBA32` | B | planned (P3) |
| 77 | `RGB24_SIGNED` | Pillow raw `RGB;8s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:560`) | `RGBA32` | B | planned (P3) |
| 78 | `RGBA32_SIGNED` | Pillow raw `RGBA;8s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:561`) | `RGBA32` | B | planned (P3) |
| 79 | `R16_SIGNED` | Pillow raw `R;16s` into mode `R` | — | — | no pad; sw — | raises `ValueError`, unrecognized image mode (`:562`) | `RGBA32` | B | planned (P3) |
| 80 | `RG32_SIGNED` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:563`) | `RGBA32` | B | planned (P3) |
| 81 | `RGB48_SIGNED` | Pillow raw `RGB;16s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:564`) | `RGBA32` | B | planned (P3) |
| 82 | `RGBA64_SIGNED` | Pillow raw `RGBA;16s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:565`) | `RGBA32` | B | planned (P3) |

The Tier B count is 22, as `DESIGN.md` section 3 says: `BC6H`; the half and float formats `RHalf`, `RGHalf`, `RFloat`, `RGFloat`, `RGBAFloat`, `ARGBFloat`, `RGBFloat`; the eight `_SIGNED` raw formats; `RG16`, `RG32`, `R16`, `RGBA64`, `RGB9e5Float` and `YUY2`. `RGBAHalf` and `RGB48` return an image, so they are not Tier B.

Two more behaviours of `parse_image_data` apply to every format: a zero width or height returns an empty RGBA image, and empty data raises `ValueError` (`:265-269`); a format number outside the enum raises `ValueError` (`:271-272`). Through `Texture2D.image` neither is reached for a texture with no data, which raises while its data is fetched (section 14.2).

## 18. Writing

`DESIGN.md` section 3 sets the rules: object re-save byte-equal; file and bundle save semantically equal and idempotent, with an optional UnityPy-layout mode byte-equal to `save(packer='none')`; compressed packers equal after decompression, byte-equal LZ4HC only with liblz4 HC level 9; `get_writeable_cab` with UnityPy's naming and GUID rule; `set_image` equal after re-decode; pre-v10 typetree writing Tier C.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| object re-save | `ObjectReader.save_typetree` (section 8); an object holding a ReferencedObject of empty class raises `KeyError` `'data'` (section 9.1, `TypeTreeHelper.write_value`) | A; B for the empty-class reference | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element; `test_parity_rows.py` an_empty_managed_reference_does_not_resave | verified P0 (L3); impl P3 |
| `SerializedFile.save` | metadata: version string (v ≥ 7), platform (v ≥ 8), typetree flag (v ≥ 13), types, `big_id_enabled` (7 to 13), objects sorted by path id with each data block aligned to 8 (`files/SerializedFile.py:415-419`), script types (v ≥ 11), externals, ref types (v ≥ 20), user information (v ≥ 5); `packer` is ignored (`files/SerializedFile.py:389-440`) | A | L4 | planned: save suite | planned (P3) |
| header, v ≥ 9 | big-endian header; data offset aligned to 16; v ≥ 22 writes 0 into the legacy metadata size, file size and data offset (the version field keeps the version), then the endian byte, the reserved bytes, the u32 metadata size, the 64-bit file size and data offset, and the file's own unknown i64 back (`files/SerializedFile.py:443-478`) | A | L4 | planned: save suite | planned (P3) |
| header, v < 9 | data right after the 16-byte header, then the endian byte and the metadata, with 32 in the data offset field (`files/SerializedFile.py:480-491`), although the data starts at 16 and the reader adds that field to every object's start (`files/ObjectReader.py:86`). Reachable only with `config.SERIALIZED_FILE_PARSE_TYPETREE` false, since below v10 the tree writer raises (next rows), and then the file lacks the trees a pre-13 reader expects. Part of pre-v10 writing. | C | L4 | planned: divergence test | planned (P3) |
| `SerializedType.write` | class id, stripped flag (v ≥ 16), script index (v ≥ 17), script id and hash (v ≥ 13), then, only when the tree flag is true (`files/SerializedFile.py:179`), the tree and, from v21, the class names (ref types) or dependencies (`files/SerializedFile.py:186-198`) (`files/SerializedFile.py:151-197`) | A | L4 | planned: save suite | planned (P3) |
| tree as a blob (v ≥ 12, v = 10) | `TypeTreeNode.dump_blob`; the common string at offset 0 (`AABB`) is written as a local string (`helpers/TypeTreeNode.py:237-278`, `:250`) | A, C | L4 | planned: divergence test | planned (P3) |
| pre-v10 and v11 typetree writing | calls `serialized_file.dump`, which does not exist, so it raises `AttributeError` (`files/SerializedFile.py:182-184`); below v13 every file carries trees (`files/SerializedFile.py:241`), so every save of such a file raises unless `config.SERIALIZED_FILE_PARSE_TYPETREE` is false; `TypeTreeNode.dump` itself writes the root's fields for every node (`helpers/TypeTreeNode.py:213-235`) | C | L4 | planned: divergence test | planned (P3) |
| `ObjectReader.write` | object table row; data from `set_raw_data` or the original bytes (`files/ObjectReader.py:132-179`) | A | L4 | planned: save suite | planned (P3) |
| `BundleFile.save` | header, then: `UnityArchive` raises `NotImplementedError` (unreachable: no `BundleFile` of that signature can be built, section 5.1); UnityWeb and UnityRaw format 6 use `save_fs(64, 64)`, other formats `save_web_raw`; UnityFS by packer: `none` (default) `save_fs(64, 64)`, so the header and every block carry the flags `0x40` (`helpers/CompressionHelper.py:172-173`); `original` the file's own flags, its first block's flags for every block, and `AttributeError` for a bundle with no blocks, which never set them (`files/BundleFile.py:167-168`, `:226`); `lz4` `(194, 2)` (blocks info LZ4 at the end, LZ4 blocks), `lzma` `(65, 1)`, a tuple passed as `(data_flag, block_info_flag)` although the docstring says the reverse, anything else `NotImplementedError` (`files/BundleFile.py:188-236`) | A; C for `original` with no blocks | L4 | `test_parity_rows.py` bundle_save_flags; planned: save suite | pinned P0; impl P3 |
| `BundleFile.save_fs` | `data_flag` is the header's flags (layout bits and the blocks-info codec) and `block_info_flag` the data blocks' codec, despite the names; directory and data rebuilt from `files`; the encryption mask of the file's flag set is XORed out of both flags when any of its bits is set (`files/BundleFile.py:303-307`), so with `ArchiveFlags` (`0x1400`) a flag holding only `0x1000` comes out holding `0x400`, which reads back as encrypted, and the data blocks' own `0x100` bit is never cleared (section 19.2); `CompressionHelper.chunk_based_compress` (`files/BundleFile.py:309`); `0x40` is required (`files/BundleFile.py:325-326`); 16-byte alignment after the header when the read set `_uses_block_alignment` (format ≥ 7, or a 2019.4.15+ engine), then the blocks info before or after the data (`0x80`) with 16-byte padding for `0x200`; the size patched in (`files/BundleFile.py:238-393`) | A, C (proposed, section 19.2) | L4 | planned: save suite | planned (P3) |
| `BundleFile.save_web_raw` | formats 1 to 3 only; above 3 raises `NotImplementedError` (`files/BundleFile.py:395-487`, `:408-409`) | A | L4 | planned: Editor fixture | planned (P4) |
| compressed packers | `CompressionHelper.compress_lz4` is liblz4 HC level 9, so `lz4` output is byte-equal only with that level (section 6) | A | L4 | planned: save suite | planned (P3) |
| `WebFile.save` | `UnityWebData1.0` table and data, then `gzip`, `brotli` or none; the default `packer="none"` never falls back to the file's own packer (`files/WebFile.py:61`, `:67-68`); the signature is the argument's, whatever the file's was (`TuanjieWebData` included); an `EndianBinaryWriter` entry (from `get_writeable_cab`) raises `AttributeError` (`files/WebFile.py:58-112`, `:71`) | A | L4 | planned: save suite | planned (P3) |
| `File.get_writeable_cab` | default name `cab_file`; returns an existing writer, raises `ValueError` for an existing non-writer; copies `flags` and endian from the first `.resS`, else flags 0 (`files/File.py:92-121`) | A | L4 | planned: save suite | planned (P3) |
| `SerializedFile.get_writeable_cab` | `None` unless the parent is a bundle or web file; path `archive:/{name}/{cab}`; registers an external once, GUID `uuid1().urn[-16:]` as ASCII (the last three hex digits of the clock sequence, a hyphen and the 12-digit node id), type 0, empty temp path (`files/SerializedFile.py:362-387`) | A | L4 | planned: save suite (the GUID rule, not its bytes) | planned (P3) |
| `Texture2D.set_image` | the target defaults to `m_TextureFormat`; paths and streams are opened with Pillow; `image_to_texture2d` with the reader's platform and `m_PlatformBlob`; mips halve until a side would drop below 4, each resized BICUBIC from the previous level and written with platform 0, so never Switch-swizzled; sets `m_Width`, `m_Height`, `m_MipMap`, `m_MipCount`, `image_data`, `m_CompleteImageSize` (all levels), `m_TextureFormat`, and empties `m_StreamData` (`classes/legacy_patch/Texture2D.py:14-60`) | A | L5 | planned: `set_image` round trip | planned (P3) |
| `Environment.save`, `File.mark_changed`, `ObjectReader.set_raw_data` | sections 3, 5.2 and 8 | A | L4 | planned: save suite | planned (P3) |

## 19. Divergences

### 19.1 Tier C (`DESIGN.md` section 3)

| divergence | UnityPy 1.25.2 | unityrs | level | test | status |
|---|---|---|---|---|---|
| RGBAHalf returns garbage | three halves read per pixel for four stored, `int(h * 256)` truncated, alpha dropped; a value at 1.0 or above, at -1/256 or below, or NaN raises `ValueError`, an infinity `OverflowError`, so most HDR data raises (`export/Texture2DConverter.py:507`, `:435-447`) | half floats to RGBA | L5 | planned: divergence test | planned (P3) |
| ASTC_HDR is wrong | HDR blocks decoded with the LDR profile into UNORM8 (`export/Texture2DConverter.py:549-554`, `:371-377`) | HDR profile | L5 | planned: divergence test | planned (P3) |
| the UV offset | `uvSrcOffset = uvDim * m_VertexCount` assigns instead of adding, so the third and later UV channels of a compressed mesh start at the wrong offset (`helpers/MeshHelper.py:504`) | `+=` | L6 | planned: divergence test (expected-divergence mask) | planned (P2a) |
| UNorm/SNorm not normalised | UNorm and SNorm channels, and the byte colour formats (`kChannelFormatColor`, `kVertexFormatColor`), are read as raw integers (`enums/VertexFormat.py:46-47`, `:54-58`, `:70-73`, `helpers/MeshHelper.py:392`), while a compressed mesh's colours come out divided by 255 (`helpers/MeshHelper.py:618-628`) | normalised to [0, 1] and [-1, 1] | L6 | planned: divergence test | planned (P1) |
| mesh endianness | `MeshHandler` defaults to `<` and no caller passes the file's endianness, so big-endian vertex data is swapped wrongly (`helpers/MeshHelper.py:73`, `:99`, `:363`; callers `export/MeshExporter.py:18`, `export/SpriteHelper.py:117`) | the file's endianness | L6 | planned: divergence test | planned (P4) |
| the 4th bone weight | `1 - sum` with `sum` counted in 31sts, so the fourth weight is far below zero (`helpers/MeshHelper.py:604`, `:587`, `:591`) | `(31 - sum) / 31` | L6 | planned: divergence test | planned (P2a) |
| ETC2_RGBA1 punch-through transparency | a pixel the punch-through rule makes transparent keeps its RGB and gets alpha 0 (texture2ddecoder 1.0.5, through `export/Texture2DConverter.py:531`); the Khronos Data Format Specification 1.3, section 22.9, makes it (0, 0, 0, 0) | (0, 0, 0, 0) | L5 | `test_texture_matrix.py` pixels_match_the_matrix pins UnityPy's side; unityrs's: planned divergence test | planned (P3) |
| ReferencedObject arrays do not skip None | the array path reads `ReferencedObjectData` with no `None` check, unlike the single-value path (`helpers/TypeTreeHelper.py:300-301` against `:201-203`); the pure-Python reader only | skips | L2 | planned: divergence test (pure-Python path) | planned (P3) |
| dump_blob's common-string offset 0 | `if common_offset:` treats offset 0 (`AABB`) as absent and writes it locally (`helpers/TypeTreeNode.py:250`) | offset 0 is common | L4 | planned: divergence test | planned (P3) |
| MeshRendererExporter fails on Unity 6 | `hasattr` is always true on the merged attrs classes, so an unbatched renderer reaches `min(None)` (`TypeError`) and a renderer without `m_StaticBatchInfo` reads `None.subMeshCount` (`AttributeError`) (`export/MeshRendererExporter.py:56-59`); `export_material` raises `AttributeError` for any material holding `_Color`, `_SColor` or `_SpecularColor`, because `clt` reads `color.R` and `ColorRGBA` has `r g b a` (`:110`), and iterates the dict `texEnvs` as pairs, so any material with a texture property raises `ValueError` (`:117`, `:168`); all three checked in the oracle on 2026-10-28 | exports | L6 | planned: divergence test | planned (P3) |
| ShaderConverter fails on 6000.3 | `ShaderSubProgram` knows sub-program versions up to 202012090, local keywords only in `[201806140, 202012090)` (`export/ShaderConverter.py:577-605`, `:598`), and a property of type Int (5, Unity 2021.1 and later) raises `ValueError` (`export/ShaderConverter.py:369`). On the corpus game (Unity 6000.3; 980 Shader objects, all of their 89,868 sub-programs at blob version 202012090, no Int properties), 946 exports return text and 34 raise `struct.error` reading keywords (`export/ShaderConverter.py:596`), run in the oracle on 2026-10-28; whether the 946 texts are right was not checked. | 6000.3 layout | L7 | planned: divergence test | planned (P3) |
| pre-v10 typetree writing (`DESIGN.md` section 3, Writing) | below v10 and at v11, `SerializedType.write` calls `serialized_file.dump`, which does not exist (`files/SerializedFile.py:182-184`); `TypeTreeNode.dump` writes the root's fields for every node (`helpers/TypeTreeNode.py:218-233`); the v < 9 header names data offset 32 for data at 16 (`files/SerializedFile.py:488-489`); section 18 | writes the old format | L4 | planned: divergence test | planned (P3) |
| the Sprite cache key ignores the FileID | the decoded texture is cached in the sprite's file by `path_id` alone, so two textures with one path id in different files share an entry (`export/SpriteHelper.py:48`, `:55`) | keyed by file and path id | L5 | planned: divergence test | planned (P1) |

### 19.2 Found while writing this inventory

These are not in `DESIGN.md` section 3. Each has a proposed tier; `DESIGN.md` decides.

| defect | UnityPy 1.25.2 | proposed | level | status |
|---|---|---|---|---|
| RGB48 decodes wrong | Pillow's `RGB;16` is packed 5-6-5, two bytes per pixel, so a pure red `(0xFFFF, 0, 0)` decodes as white (`export/Texture2DConverter.py:556`) | C | L5 | planned (P3) |
| `set_image` to BC7 raises | `etcpak.compress_bc7(data, w, h, None)`; etcpak 0.9.14 rejects `None` parameters with `ValueError` (`export/Texture2DConverter.py:95`) | B | L5 | planned (P3) |
| `set_image` to ETC_RGB4Crunched | writes plain ETC1 tagged `ETC_RGB4Crunched`, which the reader then sends to crunch (`export/Texture2DConverter.py:160-165`) | C | L5 | planned (P3) |
| Texture2DArray 8-bit integer formats | `GRAPHICS_TO_TEXTURE_MAP` maps the 8-bit UInt and SInt graphics formats to the 16-bit texture formats (`enums/GraphicsFormat.py:192-199`) | C | L5 | planned (P3) |
| Texture2DArray platform | slices are decoded with platform 0 and no blob, so Switch and Xbox arrays are not deswizzled or swapped (`classes/legacy_patch/Texture2DArray.py:33-40`) | C | L5 | planned (P3) |
| `set_image` mips | mips are written with platform 0, so a Switch texture gets a swizzled top level and linear mips (`classes/legacy_patch/Texture2D.py:44`) | C | L5 | planned (P3) |
| `str(UnityVersion)` | drops the build number above 5.x, and the type and its number at 5.x and below (`helpers/UnityVersion.py:90-94`) | A (API) | API | planned (P3) |
| `GameObject.m_Components` | an empty list raises `IndexError`; the old tuple form reads `m_GameObject` from each component pointer (`classes/legacy_patch/GameObject.py:9-15`) | C | API | planned (P3) |
| `WebFile.save` | an `EndianBinaryWriter` entry raises `AttributeError`; the default packer never reuses the file's own (`files/WebFile.py:61`, `:67-68`, `:71`) | C | L4 | planned (P3) |
| `BundleFile.save` tuple | the docstring says `(block_info_flag, data_flag)`, the code takes `(data_flag, block_info_flag)` (`files/BundleFile.py:193-194`, `:232-233`) | A (code) | L4 | planned (P3) |
| vertex-less mesh export | `export_mesh_obj` returns `False` (`export/MeshExporter.py:22-23`, `:29-30`), so `extractor.exportMesh` raises on `f.write(False)` | A | L6 | planned (P3) |
| one audio sample | `extractor.exportAudioClip` writes a single sample as `.wav` whatever its format (`tools/extractor.py:239-241`) | A | L9 | planned (P3) |
| `ContainerHelper.path_dict` | keyed by path id alone, last entry wins (`helpers/ContainerHelper.py:28`, `:47`) | A | L1 | planned (P1) |
| Texture2DArray streamed data | `image_data` falls back to `m_StreamData` only when it `is None`, but a parsed array's empty TypelessData is `b""`, so a streamed array never reads its `.resS` and every slice raises `ValueError` (`classes/legacy_patch/Texture2DArray.py:20-21`; `Texture2D.get_image_data` tests truthiness, `classes/legacy_patch/Texture2D.py:64`); checked in the oracle on 2026-10-28 | C | L5 | planned (P3) |
| OBJ NaN and infinity | `{:.9G}` prints `NAN` and `INF`, so `.replace("nan", "0")` never fires and the OBJ holds `NAN` (`export/MeshExporter.py:33`, `:39`, `:45`); checked in the oracle on 2026-10-28 | A (the text) | L6 | planned (P3) |
| `BundleFile.save_fs` encryption flags | the flag set's whole encryption mask is XORed out when any bit of it is set, so an `ArchiveFlags` header with only `0x1000` is saved with `0x400`, which UnityPy reads back as encrypted; the data blocks' `0x100` bit survives the `original` packer (`files/BundleFile.py:303-307`, `files/BundleFile.py:123-124`) | C | L4 | planned (P3) |
| `SerializedFile.load_dependencies` | `load_file(path, True)` sets `parent`, not `is_dependency`, so dependencies it loads (a `.resS` lookup miss triggers it) join `Environment.objects` and `Environment.container` (`files/SerializedFile.py:340`, `:347`; `environment.py:100-106`) | C | L1 (env) | planned (P1) |
| `config.SERIALIZED_FILE_PARSE_TYPETREE` | promises faster loading but is applied after the trees are parsed; its only effect is that `save` drops every tree and writes the flag false (`files/SerializedFile.py:310-311`, `:179`, `:405`) | A (save) | L4 | planned (P3) |
| extractor crashes | `exportGameObject` always raises `AttributeError` in `crawl_obj` (`tools/extractor.py:350`), and GameObjects sort first, so a container holding one exports nothing; `exportMonoBehaviour` writes its JSON and then raises `AttributeError` on `obj.path_id` (`tools/extractor.py:224`), and with `export_unknown_as_typetree` it raises before writing; `exportSprite` has no default extension, so every sprite of the no-container mode raises `ValueError` (`tools/extractor.py:250-259`); nothing in `extract_assets` catches, so the first raise ends the run. Checked in the oracle, on UnityPy's samples where they apply, on 2026-10-28. | C | L9 | planned (P3) |
| extractor file names | `append_path_id` appends `_None` (`tools/extractor.py:75`); images, meshes, shaders and text keep the container's extension, which picks Pillow's image format (`.tga` writes TGA, `.psd` raises); several audio samples are written `x.wav.wav`; a later container entry with the same path overwrites an earlier one; the alpha texture is never returned as exported (`tools/extractor.py:264-267`) | A (the names), C (the raises) | L9 | planned (P3) |

## 20. HEAD tier

The HEAD oracle is UnityPy git `417d595`. Lines below are in that tree. unityrs matches HEAD's dicts on Unity 6000.4/6000.5 fixtures (`DESIGN.md` section 3).

| item | UnityPy 1.25.2 | HEAD `417d595` | level | test | status |
|---|---|---|---|---|---|
| SerializedFile v23 | not read | per-type `TypeTreeNodeInfoHandler` in place of the blob (`files/SerializedFile.py:141-144`, `helpers/TypeTreeNode.py:342-394`) | L1, L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| SerializedFile v24, v25 | not read | no gates of their own | L1, L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| SerializedFile v26 | not read | shared subtrees after the ref types, resolved by hash (`files/SerializedFile.py:331-338`) | L1, L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| bundle format 9 | not read | a u64 offset per block, used to seek each block (`helpers/CompressionHelper.py:158-172`, `files/BundleFile.py:485-491`) | L1 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| `lz4hc` packer | absent | `save_fs(195, 3)` (`files/BundleFile.py:228-229`) | L4 | planned: save suite | planned (P3) |

## 21. Keeping this file complete

`tools/parity/inventory.py` lists UnityPy's public surface and checks that this file names all of it:

```sh
scripts/dev python tools/parity/inventory.py list [--json]
scripts/dev python tools/parity/inventory.py check
```

| group | what | named here as |
|---|---|---|
| modules | every `.py` file under `UnityPy/` (78) | its path in a code span, alone or with `:line`; a path names its module and nothing else |
| classes | every public class a module defines (955, the generated classes included) | its name in a code span |
| functions | every public module-level function (126) | `Module.function` in a code span |
| data | every public name a module assigns at its top level whose value is not a class, function, module or typing construct: tables, registries, settings, caches (65) | `Module.NAME` in a code span |
| methods | every public callable or property a class defines in its own body, for every class UnityPy defines; for the generated classes, the members a `classes/legacy_patch` module adds (228) | `Class.member` in a code span, or a compact row (below) |
| fields | every public attrs field a class declares itself, for every attrs class UnityPy defines, the 869 generated classes included (8,456) | `Class.field` in a code span, or a compact row: Appendix A for the generated classes, Appendix B for the rest |
| exports | every public name a package's `__init__.py` imports from another UnityPy module or assigns as an alias (924) | the dotted path from `UnityPy` (`UnityPy.files.File`), or a compact row: section 2.1 |
| boost | every public name of UnityPyBoost (4) | `UnityPyBoost.name` in a code span |
| texture formats | every `TextureFormat` member (78) | a table row with the cells `` `Name` `` and its value |

**Whole chains.** A dotted name counts only when it is written from the start of its chain. `Wrong.ImportHelper.check_file_type` does not name `ImportHelper.check_file_type`; the only prefixes allowed are the symbol's own import path (`UnityPy.helpers.ImportHelper.check_file_type`) or a package path that re-exports it (`UnityPy.classes.GameObject.m_Name`), and the only suffixes are what follows the chain (a call, a subscript, a further `.attribute`).

**Compact rows.** In a table with a column headed `fields` or `members`, a row whose first cell's first code span is `` `Parent` `` names `Parent.x` for every code span `` `x` `` in that column, and nothing under any other parent.

**Out of scope.** Enum members and other class-level constants are named by their class (the `TextureFormat` members have their own group), and private names are not inventoried.

`check` exits 1 and lists what is missing. `tools/parity/tests/test_inventory.py` runs it against this file in the oracle image, requires it to fail when one entry of any group is removed or a name is written under a wrong parent, and tests the matching rules with the standard library alone; `tools/parity/tests/test_texture_matrix.py` checks the TextureFormat matrix against the oracle (section 17). Section 23 holds the module-level data that no other section names.

### Reproducing the numbers

The group sizes above come from `scripts/dev python tools/parity/inventory.py list --counts` in the oracle image. The 869 generated classes, and the 869 rows of Appendix A, are counted by `test_inventory.py` appendix_a_has_one_row_per_generated_class.

## 22. Module index

Every module of UnityPy 1.25.2, with the sections that cover it.

| module | what | sections |
|---|---|---|
| `__init__.py` | `load`, `AssetsManager`, `__version__`, `set_assetbundle_decrypt_key` | 2, 12 |
| `__main__.py` | `python -m UnityPy` | 2 |
| `classes/__init__.py` | re-exports the classes | 2, 14 |
| `classes/ClassIDTypeToClassMap.py` | class id to class | 14.1 |
| `classes/Object.py` | `Object` | 14.1 |
| `classes/PPtr.py` | `PPtr` | 14.1 |
| `classes/UnknownObject.py` | `UnknownObject` | 14.1 |
| `classes/generated.py` | the TPK-built classes | 14, Appendix A |
| `classes/math.py` | vectors, quaternion, matrices, colour | 14.1 |
| `classes/legacy_patch/__init__.py` | applies the patches | 14.2 |
| `classes/legacy_patch/AudioClip.py` | `AudioClip.extension`, `AudioClip.samples` | 14.2, 16 |
| `classes/legacy_patch/GameObject.py` | `GameObject.m_*` | 14.2 |
| `classes/legacy_patch/Mesh.py` | `Mesh.export` | 14.2 |
| `classes/legacy_patch/Renderer.py` | `Renderer.export` | 14.2 |
| `classes/legacy_patch/Shader.py` | `Shader.export` | 14.2 |
| `classes/legacy_patch/Sprite.py` | `Sprite.image` | 14.2 |
| `classes/legacy_patch/Texture2D.py` | `Texture2D.image`, `set_image`, `get_image_data` | 14.2, 18 |
| `classes/legacy_patch/Texture2DArray.py` | `Texture2DArray.images` | 14.2 |
| `cli/__init__.py` | the console entry | 2 |
| `cli/update_tpk.py` | downloads a TPK | 2, 10 |
| `config.py` | version fallback, serialized-file parse flag | 10 |
| `environment.py` | `Environment` | 3 |
| `exceptions.py` | the exception classes | 10 |
| `enums/__init__.py` | re-exports the enums | 2 |
| `enums/Audio.py` | audio enums and extensions | 16 |
| `enums/BuildTarget.py` | platforms | 7, 15.1 |
| `enums/BundleFile.py` | archive and block flags | 5.1 |
| `enums/ClassIDType.py` | class ids | 14.1 |
| `enums/CommonString.py` | a fixed common-string table, read only by the never-called `SerializedFile.read_string` | 7.2, 23 |
| `enums/ExtendableEnum.py` | enum base with unknown members | 14.1 |
| `enums/FileType.py` | detection result | 4 |
| `enums/GfxPrimitiveType.py` | primitive types | 15.3 |
| `enums/GraphicsFormat.py` | graphics formats and the array map | 14.2, 19.2 |
| `enums/MeshTopology.py` | topology | 15.3 |
| `enums/PassType.py` | shader passes | 15.5 |
| `enums/SerializedPropertyType.py` | shader properties | 15.5 |
| `enums/ShaderCompilerPlatform.py` | shader platforms | 15.5 |
| `enums/ShaderGpuProgramType.py` | shader program types | 15.5 |
| `enums/SpriteMeshType.py` | sprite mesh type | 15.2 |
| `enums/SpritePackingMode.py` | sprite packing mode | 15.2 |
| `enums/SpritePackingRotation.py` | sprite rotation | 15.2 |
| `enums/TextureDimension.py` | texture dimension | 15.5 |
| `enums/TextureFormat.py` | texture formats | 17 |
| `enums/VertexFormat.py` | vertex formats and struct codes | 15.3, 19.1 |
| `export/__init__.py` | re-exports the exporters | 2 |
| `export/AudioClipConverter.py` | audio samples | 16 |
| `export/MeshExporter.py` | OBJ | 15.4 |
| `export/MeshRendererExporter.py` | OBJ and MTL for renderers | 15.4, 19.1 |
| `export/ShaderConverter.py` | shader text | 15.5 |
| `export/SpriteHelper.py` | sprites | 15.2 |
| `export/Texture2DConverter.py` | texture decode and encode | 15.1, 17 |
| `files/__init__.py` | re-exports the file classes | 2 |
| `files/BundleFile.py` | bundles | 5.1, 18 |
| `files/File.py` | the container base | 5.2, 18 |
| `files/ObjectReader.py` | objects | 8 |
| `files/SerializedFile.py` | serialized files | 7, 18 |
| `files/WebFile.py` | `UnityWebData` files | 5.2, 18 |
| `helpers/__init__.py` | package marker | 2 |
| `helpers/ArchiveStorageManager.py` | UnityCN | 12 |
| `helpers/CompressionHelper.py` | codecs | 6 |
| `helpers/ContainerHelper.py` | containers | 14.3 |
| `helpers/ImportHelper.py` | detection and split files | 4 |
| `helpers/MeshHelper.py` | mesh decoding | 15.3, 19.1 |
| `helpers/PackedBitVector.py` | packed bits | 15.3 |
| `helpers/ResourceReader.py` | `.resS` data | 5.2 |
| `helpers/TextureSwizzler.py` | Switch swizzling | 15.6 |
| `helpers/Tpk.py` | the TPK | 10 |
| `helpers/TypeTreeGenerator.py` | MonoBehaviour trees | 10 |
| `helpers/TypeTreeHelper.py` | typetree values | 9 |
| `helpers/TypeTreeNode.py` | typetree nodes | 9.3, 18 |
| `helpers/UnityVersion.py` | versions | 11 |
| `resources/__init__.py` | bundled resources | 2, 10 |
| `streams/__init__.py` | re-exports the streams | 2, 13 |
| `streams/EndianBinaryReader.py` | readers | 13 |
| `streams/EndianBinaryWriter.py` | writer | 13 |
| `tools/__init__.py` | package marker | 2 |
| `tools/TpkClassGenerator.py` | builds `classes/generated.py` | 10, 14 |
| `tools/extractor.py` | the extractor | 15.7 |

## 23. Module-level data

Public tables, registries, settings and caches that no other section names (section 21, group `data`). Those named elsewhere: `config.FALLBACK_UNITY_VERSION` and `config.SERIALIZED_FILE_PARSE_TYPETREE` (sections 10 and 7.2), `environment.reSplit` (3), the `CompressionHelper` tables and magics (6), `ArchiveStorageManager.DECRYPT_KEY` and `ArchiveStorageManager.UNITY3D_SIGNATURE` (12), `Texture2DConverter.CONV_TABLE`, `Texture2DConverter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE`, `Texture2DConverter.RG_PADDING_MAP` and `Texture2DConverter.XBOX_SWAP_FORMATS` (15.1, 17), `TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP` (17), `GraphicsFormat.GRAPHICS_TO_TEXTURE_MAP` (14.2), the `VertexFormat` struct maps (15.3), `ShaderConverter.HEADER` (15.5), `Audio.AUDIO_TYPE_EXTEMSION` (16), `EndianBinaryReader.reNot0` (13), `extractor.EXPORT_TYPES` and `extractor.MONOBEHAVIOUR_TYPETREES` (15.7). The mutable tables among them (`CONV_TABLE`, `EXPORT_TYPES`, the codec maps) are extension points a UnityPy user may patch; the Python bindings (P3) decide which to mirror.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TypeTreeHelper.FUNCTION_READ_MAP`, `TypeTreeHelper.FUNCTION_READ_MAP_ARRAY`, `TypeTreeHelper.FUNCTION_WRITE_MAP` | the primitive type names the pure-Python reader and writer know, `SInt8` to `TypelessData` with `Type*` and `FileSize`, and their readers and writers; the array table lacks `string` and `TypelessData`, which `read_value_array` handles itself (`helpers/TypeTreeHelper.py:29-75`, `:381-404`) | A | L2, L3 | `test_unitypy_boost.py` (all); `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L2, L3); impl P1 |
| `TypeTreeHelper.kAlignBytes` | `0x4000`, the flag `TypeTreeHelper.metaflag_is_aligned` tests (`helpers/TypeTreeHelper.py:27`) | — | L2 | — | n/a |
| `TypeTreeNode.TYPETREENODE_KEYS` | the field order of `TypeTreeNode.to_dict` (`helpers/TypeTreeNode.py:52-64`) | — | API | — | n/a |
| `TypeTreeNode.NAME_PEEK_NODE_CACHE`, `TypeTreeNode.SYSTEM_GLOBAL_LOCK` | the process-wide cache of `ObjectReader.peek_name`, keyed by the root's `(m_Name, m_Type, m_Version)` and filled under the lock (`helpers/TypeTreeNode.py:66-67`, `:190-211`) | — | API | — | n/a |
| `TypeTreeNode.COMMONSTRING_CACHE` | `TypeTreeNode.get_common_strings` results by version (`helpers/TypeTreeNode.py:305-327`) | — | L1 | — | n/a |
| `CommonString.CommonString` | a fixed offset-to-string table read only by the never-called `SerializedFile.read_string`; blob trees use the TPK's strings (`enums/CommonString.py`, `files/SerializedFile.py:496-503`) | — | — | — | n/a |
| `ClassIDTypeToClassMap.ClassIDTypeToClassMap` | the class map of section 14.1 (`classes/ClassIDTypeToClassMap.py:720`) | A | API | planned: binding | planned (P3) |
| `Tpk.TPKTYPETREE`, `Tpk.CLASSES_CACHE`, `Tpk.NODES_CACHE` | the loaded TPK type-tree blob, and the node caches of `Tpk.get_typetree_node` and `Tpk.generate_node` (`helpers/Tpk.py:15-17`) | — | L2b | — | n/a |
| `Tpk.BYTE`, `Tpk.UINT16`, `Tpk.INT32`, `Tpk.INT64`, `Tpk.UINT64` | the TPK reader's structs, signed byte and little-endian integers (`helpers/Tpk.py:438-442`) | — | L2b | — | n/a |
| `TextureSwizzler.GOB_X_TEXEL_COUNT`, `TextureSwizzler.GOB_Y_TEXEL_COUNT`, `TextureSwizzler.TEXEL_BYTE_SIZE`, `TextureSwizzler.TEXELS_IN_GOB`, `TextureSwizzler.GOB_MAP` | the Switch GOB: 4×8 texels of 16 bytes, and the order of the 32 texels inside it (`helpers/TextureSwizzler.py:6-10`) | A | L5 | planned: Switch fixture | planned (P3) |
| `EndianBinaryReader.SYS_ENDIAN`, `EndianBinaryReader.TYPE_PARAM_SIZE_LIST`, `EndianBinaryReader.MEMORY_FUNCTIONS`, `EndianBinaryReader.STREAM_FUNCTIONS` | the host's byte order and the generated primitive readers by endian (`streams/EndianBinaryReader.py:13-30`, `:335-349`) | — | — | — | n/a |
| `UnityVersion.VersionPattern` | the pattern of `UnityVersion.from_str` (`helpers/UnityVersion.py:7-10`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `BundleFile.reVersion` | a version pattern nothing reads (`files/BundleFile.py:15`) | — | — | — | n/a |
| `update_tpk.URL`, `update_tpk.RESOURCE_PATH` | where `update_tpk` downloads AssetRipper's TPK, and the folder it unpacks `lzma.tpk` into (`cli/update_tpk.py:8-9`) | — | — | — | n/a |
| `TpkClassGenerator.ROOT`, `TpkClassGenerator.NODES`, `TpkClassGenerator.STRINGS`, `TpkClassGenerator.BASE_TYPE_MAP`, `TpkClassGenerator.GENERATED_HEADER`, `TpkClassGenerator.MATH_CLASSES`, `TpkClassGenerator.FORBIDDEN_CLASSES`, `TpkClassGenerator.CLASS_CACHE_ID`, `TpkClassGenerator.CLASS_CACHE_NAME`, `TpkClassGenerator.TYPE_CACHE` | the class generator's settings and caches (`tools/TpkClassGenerator.py:11-129`) | — | — | — | n/a |

## Appendix A. Generated classes

Every class in `classes/generated.py` (869), with its bases, the class ids `classes/ClassIDTypeToClassMap.py` maps to it, and the attrs fields it declares (`inventory.py` group `fields`; a generated class declares every one of its fields itself, so none is inherited). They are read through the typetree: Tier A at L2 through the dict (verified in P0 wherever the samples and the corpus hold them), and at the API level in object mode (planned P3, section 9.2).

| class | bases | class ids | fields |
|---|---|---|---|
| `AnnotationManager` | Object | 1049 | `m_CurrentPreset_m_AnnotationList`, `m_RecentlyChanged`, `m_FadeGizmoSize`, `m_FadeGizmos`, `m_IconSize`, `m_ShowGrid`, `m_ShowLODLabels`, `m_ShowSelectionOutline`, `m_ShowSelectionWire`, `m_Use3dGizmos`, `m_WorldIconSize` |
| `AssetDatabaseV1` | Object | 1004 | `m_AssetBundleNames`, `m_AssetTimeStamps`, `m_Assets`, `m_Metrics`, `m_UnityShadersVersion`, `m_lastValidVersionHashes`, `m_lastValidVersions` |
| `AssetMetaData` | Object | 1028 | `assetStoreRef`, `guid`, `labels`, `originalName`, `pathName`, `licenseType`, `originalChangeset`, `originalDigest`, `originalParent`, `timeCreated` |
| `AssetServerCache` | Object | 1037 | `m_CachesInitialized`, `m_CommitItemSelection`, `m_DeletedItems`, `m_Items`, `m_LastCommitMessage`, `m_LatestServerChangeset`, `m_ModifiedItems`, `m_WorkingItemMetaData` |
| `AudioBuildInfo` | Object | 641289076 | `m_AudioClipCount`, `m_AudioMixerCount`, `m_IsAudioDisabled` |
| `BlockShaderSourceArtifact` | Object | — | `shaderName`, `shaderSource` |
| `BuiltAssetBundleInfoSet` | Object | 668709126 | `bundleInfos` |
| `ContentSummary` | Object | — | `m_assetStatsList`, `m_generatedFileCount`, `m_generatedFileSize`, `m_headerSize`, `m_objectCount`, `m_resourceDataSize`, `m_resourceFileCount`, `m_serializedFileCount`, `m_serializedFileSize`, `m_sizeReusedContentInOutputDirectory`, `m_typeStatsList` |
| `Derived` | Object | 1091556383 | — |
| `SubDerived` | Derived | 367388927 | — |
| `DifferentMarshallingTestObject` | Object | — | — |
| `EditorBuildSettings` | Object | 1045 | `m_Scenes`, `m_UseUCBPForAssetBundles`, `m_configObjects` |
| `EditorExtension` | Object, ABC | 18 | — |
| `Component` | EditorExtension | 2 | `m_GameObject` |
| `Behaviour` | Component | 8 | `m_GameObject` |
| `Animation` | Behaviour | 111 | `m_AnimatePhysics`, `m_Animation`, `m_Animations`, `m_CullingType`, `m_Enabled`, `m_GameObject`, `m_PlayAutomatically`, `m_WrapMode`, `m_UpdateMode`, `m_UserAABB` |
| `Animator` | Behaviour | 95 | `m_ApplyRootMotion`, `m_Avatar`, `m_Controller`, `m_CullingMode`, `m_Enabled`, `m_GameObject`, `m_AllowConstantClipSamplingOptimization`, `m_AnimatePhysics`, `m_HasTransformHierarchy`, `m_KeepAnimatorControllerStateOnDisable`, `m_KeepAnimatorStateOnDisable`, `m_LinearVelocityBlending`, `m_StabilizeFeet`, `m_UpdateMode`, `m_WriteDefaultValuesOnDisable` |
| `ArticulationBody` | Behaviour | 171741748 | `m_AnchorPosition`, `m_AnchorRotation`, `m_AngularDamping`, `m_ArticulationJointType`, `m_Enabled`, `m_GameObject`, `m_Immovable`, `m_JointFriction`, `m_LinearDamping`, `m_LinearX`, `m_LinearY`, `m_LinearZ`, `m_Mass`, `m_ParentAnchorPosition`, `m_ParentAnchorRotation`, `m_SwingY`, `m_SwingZ`, `m_Twist`, `m_XDrive`, `m_YDrive`, `m_ZDrive`, `m_CenterOfMass`, `m_CollisionDetectionMode`, `m_ComputeParentAnchor`, `m_ExcludeLayers`, `m_ImplicitCom`, `m_ImplicitTensor`, `m_IncludeLayers`, `m_InertiaRotation`, `m_InertiaTensor`, `m_MatchAnchors`, `m_UseGravity` |
| `AudioBehaviour` | Behaviour | 180 | `m_GameObject` |
| `AudioListener` | AudioBehaviour | 81 | `m_Enabled`, `m_GameObject`, `m_ExtensionPropertyValues` |
| `AudioSource` | AudioBehaviour | 82 | `BypassEffects`, `DopplerLevel`, `Loop`, `MaxDistance`, `MinDistance`, `Mute`, `Pan2D`, `Priority`, `m_Enabled`, `m_GameObject`, `m_Pitch`, `m_PlayOnAwake`, `m_Volume`, `m_audioClip`, `panLevelCustomCurve`, `rolloffCustomCurve`, `rolloffMode`, `spreadCustomCurve`, `BypassListenerEffects`, `BypassReverbZones`, `OutputAudioMixerGroup`, `Spatialize`, `SpatializePostEffects`, `m_ExtensionPropertyValues`, `m_Resource`, `reverbZoneMixCustomCurve` |
| `AudioFilter` | Behaviour | 181 | `m_GameObject` |
| `AudioChorusFilter` | AudioFilter | 166 | `m_Delay`, `m_Depth`, `m_DryMix`, `m_Enabled`, `m_GameObject`, `m_Rate`, `m_WetMix1`, `m_WetMix2`, `m_WetMix3`, `m_FeedBack` |
| `AudioDistortionFilter` | AudioFilter | 170 | `m_DistortionLevel`, `m_Enabled`, `m_GameObject` |
| `AudioEchoFilter` | AudioFilter | 168 | `m_DecayRatio`, `m_Delay`, `m_DryMix`, `m_Enabled`, `m_GameObject`, `m_WetMix` |
| `AudioHighPassFilter` | AudioFilter | 165 | `m_CutoffFrequency`, `m_Enabled`, `m_GameObject`, `m_HighpassResonanceQ` |
| `AudioLowPassFilter` | AudioFilter | 169 | `lowpassLevelCustomCurve`, `m_Enabled`, `m_GameObject`, `m_LowpassResonanceQ`, `m_CutoffFrequency` |
| `AudioReverbFilter` | AudioFilter | 164 | `m_DecayHFRatio`, `m_DecayTime`, `m_Density`, `m_Diffusion`, `m_DryLevel`, `m_Enabled`, `m_GameObject`, `m_HFReference`, `m_LFReference`, `m_ReflectionsDelay`, `m_ReflectionsLevel`, `m_ReverbDelay`, `m_ReverbLevel`, `m_ReverbPreset`, `m_Room`, `m_RoomHF`, `m_RoomLF`, `m_RoomRolloff` |
| `AudioReverbZone` | Behaviour | 167 | `m_DecayHFRatio`, `m_DecayTime`, `m_Density`, `m_Diffusion`, `m_Enabled`, `m_GameObject`, `m_HFReference`, `m_LFReference`, `m_MaxDistance`, `m_MinDistance`, `m_Reflections`, `m_ReflectionsDelay`, `m_Reverb`, `m_ReverbDelay`, `m_ReverbPreset`, `m_Room`, `m_RoomHF`, `m_RoomLF`, `m_RoomRolloffFactor` |
| `Camera` | Behaviour | 20 | `far_clip_plane`, `field_of_view`, `m_BackGroundColor`, `m_ClearFlags`, `m_CullingMask`, `m_Depth`, `m_Enabled`, `m_GameObject`, `m_NormalizedViewPortRect`, `m_RenderingPath`, `m_TargetTexture`, `near_clip_plane`, `orthographic`, `orthographic_size`, `m_AllowDynamicResolution`, `m_AllowMSAA`, `m_Anamorphism`, `m_Aperture`, `m_BarrelClipping`, `m_BladeCount`, `m_Curvature`, `m_FocalLength`, `m_FocusDistance`, `m_ForceIntoRT`, `m_GateFitMode`, `m_HDR`, `m_Iso`, `m_LensShift`, `m_OcclusionCulling`, `m_SensorSize`, `m_ShutterSpeed`, `m_StereoConvergence`, `m_StereoMirrorMode`, `m_StereoSeparation`, `m_TargetDisplay`, `m_TargetEye`, `m_projectionMatrixMode` |
| `ScriptableCamera` | Camera | 488575907 | `far_clip_plane`, `field_of_view`, `m_AllowDynamicResolution`, `m_AllowMSAA`, `m_BackGroundColor`, `m_ClearFlags`, `m_CullingMask`, `m_Depth`, `m_Enabled`, `m_FocalLength`, `m_ForceIntoRT`, `m_GameObject`, `m_GateFitMode`, `m_HDR`, `m_LensShift`, `m_NormalizedViewPortRect`, `m_OcclusionCulling`, `m_RenderingPath`, `m_Script`, `m_SensorSize`, `m_StereoConvergence`, `m_StereoSeparation`, `m_TargetDisplay`, `m_TargetEye`, `m_TargetTexture`, `m_projectionMatrixMode`, `near_clip_plane`, `orthographic`, `orthographic_size` |
| `Canvas` | Behaviour | 223 | `m_Camera`, `m_Enabled`, `m_GameObject`, `m_PixelPerfect`, `m_RenderMode`, `m_AdditionalShaderChannelsFlag`, `m_Alpha`, `m_Normals`, `m_OverridePixelPerfect`, `m_OverrideSorting`, `m_PlaneDistance`, `m_PositionUVs`, `m_ReceivesEvents`, `m_SortingBucketNormalizedSize`, `m_SortingLayerID`, `m_SortingOrder`, `m_TargetDisplay`, `m_UpdateRectTransformForStandalone`, `m_UseReflectionProbes`, `m_VertexColorAlwaysGammaSpace` |
| `CanvasGroup` | Behaviour | 225 | `m_Alpha`, `m_BlocksRaycasts`, `m_GameObject`, `m_IgnoreParentGroups`, `m_Interactable`, `m_Enabled` |
| `Cloth` | Behaviour | 183 | `m_GameObject`, `m_BendingStiffness`, `m_CapsuleColliders`, `m_Coefficients`, `m_CollisionMassScale`, `m_Damping`, `m_Enabled`, `m_ExternalAcceleration`, `m_Friction`, `m_RandomAcceleration`, `m_SelfAndInterCollisionIndices`, `m_SelfCollisionDistance`, `m_SelfCollisionStiffness`, `m_SleepThreshold`, `m_SolverFrequency`, `m_SphereColliders`, `m_StretchingStiffness`, `m_UseContinuousCollision`, `m_UseGravity`, `m_UseTethers`, `m_UseVirtualParticles`, `m_VirtualParticleIndices`, `m_VirtualParticleWeights`, `m_WorldAccelerationScale`, `m_WorldVelocityScale` |
| `InteractiveCloth` | Cloth | 160 | `m_AttachedColliders`, `m_AttachmentResponse`, `m_AttachmentTearFactor`, `m_BendingStiffness`, `m_CollisionResponse`, `m_Damping`, `m_Density`, `m_Enabled`, `m_ExternalAcceleration`, `m_Friction`, `m_GameObject`, `m_Mesh`, `m_Pressure`, `m_RandomAcceleration`, `m_SelfCollision`, `m_StretchingStiffness`, `m_TearFactor`, `m_Thickness`, `m_UseGravity` |
| `SkinnedCloth` | Cloth | 163 | `m_BendingStiffness`, `m_Coefficients`, `m_Damping`, `m_Enabled`, `m_ExternalAcceleration`, `m_GameObject`, `m_RandomAcceleration`, `m_SelfCollision`, `m_StretchingStiffness`, `m_Thickness`, `m_UseGravity`, `m_WorldAccelerationScale`, `m_WorldVelocityScale` |
| `CloudServiceHandlerBehaviour` | Behaviour | — | `m_Enabled`, `m_GameObject` |
| `Collider2D` | Behaviour | 53 | `m_GameObject` |
| `BoxCollider2D` | Collider2D | 61 | `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Size`, `m_AutoTiling`, `m_CallbackLayers`, `m_Center`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_Density`, `m_EdgeRadius`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_Offset`, `m_SpriteTilingProperty`, `m_UsedByComposite`, `m_UsedByEffector` |
| `CapsuleCollider2D` | Collider2D | 70 | `m_Density`, `m_Direction`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Offset`, `m_Size`, `m_UsedByEffector`, `m_CallbackLayers`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_UsedByComposite` |
| `CircleCollider2D` | Collider2D | 58 | `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Radius`, `m_CallbackLayers`, `m_Center`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_Density`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_Offset`, `m_UsedByComposite`, `m_UsedByEffector` |
| `CompositeCollider2D` | Collider2D | 66 | `m_ColliderPaths`, `m_CompositePaths`, `m_Density`, `m_Enabled`, `m_GameObject`, `m_GenerationType`, `m_GeometryType`, `m_IsTrigger`, `m_Material`, `m_Offset`, `m_UsedByEffector`, `m_VertexDistance`, `m_CallbackLayers`, `m_CompositeGameObject`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_EdgeRadius`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_OffsetDistance`, `m_UseDelaunayMesh`, `m_UsedByComposite` |
| `CustomCollider2D` | Collider2D | — | `m_CustomShapes`, `m_Density`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Offset`, `m_UsedByEffector`, `m_CallbackLayers`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_UsedByComposite` |
| `EdgeCollider2D` | Collider2D | 68 | `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Points`, `m_AdjacentEndPoint`, `m_AdjacentStartPoint`, `m_CallbackLayers`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_Density`, `m_EdgeRadius`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_Offset`, `m_UseAdjacentEndPoint`, `m_UseAdjacentStartPoint`, `m_UsedByComposite`, `m_UsedByEffector` |
| `PolygonCollider2D` | Collider2D | 60 | `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_AutoTiling`, `m_CallbackLayers`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_Density`, `m_ExcludeLayers`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_Offset`, `m_Points`, `m_Poly`, `m_SpriteTilingProperty`, `m_UseDelaunayMesh`, `m_UsedByComposite`, `m_UsedByEffector` |
| `PolygonColliderBase2D` | Collider2D | — | `m_GameObject` |
| `TilemapCollider2D` | Collider2D | 19719996 | `m_Density`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Offset`, `m_UsedByEffector`, `m_CallbackLayers`, `m_CompositeOperation`, `m_CompositeOrder`, `m_ContactCaptureLayers`, `m_ExcludeLayers`, `m_ExtrusionFactor`, `m_ForceReceiveLayers`, `m_ForceSendLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_MaximumTileChangeCount`, `m_UseDelaunayMesh`, `m_UsedByComposite` |
| `ConstantForce` | Behaviour | 75 | `m_Enabled`, `m_Force`, `m_GameObject`, `m_RelativeForce`, `m_RelativeTorque`, `m_Torque` |
| `Effector2D` | Behaviour | 248 | `m_GameObject` |
| `AreaEffector2D` | Effector2D | 249 | `m_ColliderMask`, `m_Enabled`, `m_ForceMagnitude`, `m_ForceTarget`, `m_ForceVariation`, `m_GameObject`, `m_AngularDamping`, `m_AngularDrag`, `m_Drag`, `m_ForceAngle`, `m_ForceDirection`, `m_LinearDamping`, `m_UseColliderMask`, `m_UseGlobalAngle` |
| `BuoyancyEffector2D` | Effector2D | 253 | `m_ColliderMask`, `m_Density`, `m_Enabled`, `m_FlowAngle`, `m_FlowMagnitude`, `m_FlowVariation`, `m_GameObject`, `m_SurfaceLevel`, `m_UseColliderMask`, `m_AngularDamping`, `m_AngularDrag`, `m_LinearDamping`, `m_LinearDrag` |
| `PlatformEffector2D` | Effector2D | 251 | `m_ColliderMask`, `m_Enabled`, `m_GameObject`, `m_OneWay`, `m_RotationalOffset`, `m_SideAngleVariance`, `m_SideArc`, `m_SideBounce`, `m_SideFriction`, `m_SurfaceArc`, `m_UseColliderMask`, `m_UseOneWay`, `m_UseOneWayGrouping`, `m_UseSideBounce`, `m_UseSideFriction` |
| `PointEffector2D` | Effector2D | 250 | `m_ColliderMask`, `m_DistanceScale`, `m_Enabled`, `m_ForceMagnitude`, `m_ForceMode`, `m_ForceSource`, `m_ForceTarget`, `m_ForceVariation`, `m_GameObject`, `m_AngularDamping`, `m_AngularDrag`, `m_Drag`, `m_LinearDamping`, `m_UseColliderMask` |
| `SurfaceEffector2D` | Effector2D | 252 | `m_ColliderMask`, `m_Enabled`, `m_GameObject`, `m_Speed`, `m_SpeedVariation`, `m_ForceScale`, `m_UseBounce`, `m_UseColliderMask`, `m_UseContactForce`, `m_UseFriction` |
| `FlareLayer` | Behaviour | 124 | `m_Enabled`, `m_GameObject` |
| `GUIElement` | Behaviour | 133 | `m_GameObject` |
| `GUIText` | GUIElement | 132 | `m_Alignment`, `m_Anchor`, `m_Enabled`, `m_Font`, `m_FontSize`, `m_FontStyle`, `m_GameObject`, `m_LineSpacing`, `m_Material`, `m_PixelCorrect`, `m_PixelOffset`, `m_TabSize`, `m_Text`, `m_Color`, `m_RichText` |
| `GUITexture` | GUIElement | 131 | `m_BottomBorder`, `m_Color`, `m_Enabled`, `m_GameObject`, `m_LeftBorder`, `m_PixelInset`, `m_RightBorder`, `m_Texture`, `m_TopBorder` |
| `GUILayer` | Behaviour | 92 | `m_Enabled`, `m_GameObject` |
| `GridLayout` | Behaviour | 1742807556 | `m_GameObject` |
| `Grid` | GridLayout | 156049354 | `m_CellGap`, `m_CellLayout`, `m_CellSize`, `m_CellSwizzle`, `m_Enabled`, `m_GameObject` |
| `Tilemap` | GridLayout | 1839735485 | `m_AnimatedTiles`, `m_AnimationFrameRate`, `m_Color`, `m_Enabled`, `m_GameObject`, `m_Origin`, `m_Size`, `m_TileAnchor`, `m_TileAssetArray`, `m_TileColorArray`, `m_TileMatrixArray`, `m_TileOrientation`, `m_TileOrientationMatrix`, `m_TileSpriteArray`, `m_Tiles`, `m_TileObjectToInstantiateArray` |
| `Halo` | Behaviour | 122 | `m_Color`, `m_Enabled`, `m_GameObject`, `m_Size` |
| `HaloLayer` | Behaviour | 125 | `m_Enabled`, `m_GameObject` |
| `IConstraint` | Behaviour | 285090594 | `m_GameObject` |
| `AimConstraint` | IConstraint | 895512359 | `m_AffectRotationX`, `m_AffectRotationY`, `m_AffectRotationZ`, `m_AimVector`, `m_Enabled`, `m_GameObject`, `m_RotationAtRest`, `m_RotationOffset`, `m_Sources`, `m_UpType`, `m_UpVector`, `m_Weight`, `m_WorldUpObject`, `m_WorldUpVector`, `m_Active`, `m_IsContraintActive` |
| `LookAtConstraint` | IConstraint | 1183024399 | `m_Enabled`, `m_GameObject`, `m_Roll`, `m_RotationAtRest`, `m_RotationOffset`, `m_Sources`, `m_UseUpObject`, `m_Weight`, `m_WorldUpObject`, `m_Active`, `m_IsContraintActive` |
| `ParentConstraint` | IConstraint | 1773428102 | `m_AffectRotationX`, `m_AffectRotationY`, `m_AffectRotationZ`, `m_AffectTranslationX`, `m_AffectTranslationY`, `m_AffectTranslationZ`, `m_Enabled`, `m_GameObject`, `m_RotationAtRest`, `m_RotationOffsets`, `m_Sources`, `m_TranslationAtRest`, `m_TranslationOffsets`, `m_Weight`, `m_Active`, `m_IsContraintActive` |
| `PositionConstraint` | IConstraint | 1818360608 | `m_AffectTranslationX`, `m_AffectTranslationY`, `m_AffectTranslationZ`, `m_Enabled`, `m_GameObject`, `m_Sources`, `m_TranslationAtRest`, `m_TranslationOffset`, `m_Weight`, `m_Active`, `m_IsContraintActive` |
| `RotationConstraint` | IConstraint | 1818360609 | `m_AffectRotationX`, `m_AffectRotationY`, `m_AffectRotationZ`, `m_Enabled`, `m_GameObject`, `m_RotationAtRest`, `m_RotationOffset`, `m_Sources`, `m_Weight`, `m_Active`, `m_IsContraintActive` |
| `ScaleConstraint` | IConstraint | 1818360610 | `m_AffectScalingX`, `m_AffectScalingY`, `m_AffectScalingZ`, `m_Enabled`, `m_GameObject`, `m_ScaleAtRest`, `m_ScaleOffset`, `m_Sources`, `m_Weight`, `m_Active`, `m_IsContraintActive` |
| `Joint2D` | Behaviour | 230 | `m_GameObject` |
| `AnchoredJoint2D` | Joint2D | 229 | `m_GameObject` |
| `DistanceJoint2D` | AnchoredJoint2D | 232 | `m_Anchor`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_Distance`, `m_Enabled`, `m_GameObject`, `m_AutoConfigureConnectedAnchor`, `m_AutoConfigureDistance`, `m_BreakAction`, `m_BreakForce`, `m_BreakTorque`, `m_CollideConnected`, `m_EnableCollision`, `m_MaxDistanceOnly` |
| `FixedJoint2D` | AnchoredJoint2D | 255 | `m_Anchor`, `m_AutoConfigureConnectedAnchor`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_DampingRatio`, `m_EnableCollision`, `m_Enabled`, `m_Frequency`, `m_GameObject`, `m_BreakAction` |
| `FrictionJoint2D` | AnchoredJoint2D | 256 | `m_Anchor`, `m_AutoConfigureConnectedAnchor`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_EnableCollision`, `m_Enabled`, `m_GameObject`, `m_MaxForce`, `m_MaxTorque`, `m_BreakAction` |
| `HingeJoint2D` | AnchoredJoint2D | 233 | `m_Anchor`, `m_AngleLimits`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_Enabled`, `m_GameObject`, `m_Motor`, `m_UseLimits`, `m_UseMotor`, `m_AutoConfigureConnectedAnchor`, `m_BreakAction`, `m_BreakForce`, `m_BreakTorque`, `m_CollideConnected`, `m_EnableCollision`, `m_UseConnectedAnchor` |
| `SliderJoint2D` | AnchoredJoint2D | 234 | `m_Anchor`, `m_Angle`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_Enabled`, `m_GameObject`, `m_Motor`, `m_TranslationLimits`, `m_UseLimits`, `m_UseMotor`, `m_AutoConfigureAngle`, `m_AutoConfigureConnectedAnchor`, `m_BreakAction`, `m_BreakForce`, `m_BreakTorque`, `m_CollideConnected`, `m_EnableCollision` |
| `SpringJoint2D` | AnchoredJoint2D | 231 | `m_Anchor`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_DampingRatio`, `m_Distance`, `m_Enabled`, `m_Frequency`, `m_GameObject`, `m_AutoConfigureConnectedAnchor`, `m_AutoConfigureDistance`, `m_BreakAction`, `m_BreakForce`, `m_BreakTorque`, `m_CollideConnected`, `m_EnableCollision` |
| `WheelJoint2D` | AnchoredJoint2D | 235 | `m_Anchor`, `m_ConnectedAnchor`, `m_ConnectedRigidBody`, `m_Enabled`, `m_GameObject`, `m_Motor`, `m_Suspension`, `m_UseMotor`, `m_AutoConfigureConnectedAnchor`, `m_BreakAction`, `m_BreakForce`, `m_BreakTorque`, `m_CollideConnected`, `m_EnableCollision` |
| `RelativeJoint2D` | Joint2D | 254 | `m_AngularOffset`, `m_AutoConfigureOffset`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedRigidBody`, `m_CorrectionScale`, `m_EnableCollision`, `m_Enabled`, `m_GameObject`, `m_LinearOffset`, `m_MaxForce`, `m_MaxTorque`, `m_BreakAction` |
| `TargetJoint2D` | Joint2D | 257 | `m_Anchor`, `m_AutoConfigureTarget`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedRigidBody`, `m_DampingRatio`, `m_EnableCollision`, `m_Enabled`, `m_Frequency`, `m_GameObject`, `m_MaxForce`, `m_Target`, `m_BreakAction` |
| `LensFlare` | Behaviour | 123 | `m_Brightness`, `m_Color`, `m_Directional`, `m_Enabled`, `m_Flare`, `m_GameObject`, `m_IgnoreLayers`, `m_FadeSpeed` |
| `Light` | Behaviour | 108 | `m_Color`, `m_Cookie`, `m_CullingMask`, `m_DrawHalo`, `m_Enabled`, `m_Flare`, `m_GameObject`, `m_Intensity`, `m_Lightmapping`, `m_Range`, `m_RenderMode`, `m_Shadows`, `m_SpotAngle`, `m_Type`, `m_ActuallyLightmapped`, `m_AreaSize`, `m_BakedIndex`, `m_BakingOutput`, `m_BounceIntensity`, `m_BoundingSphereOverride`, `m_CCT`, `m_ColorTemperature`, `m_CookieSize`, `m_CookieSize2D`, `m_EnableSpotReflector`, `m_FalloffTable`, `m_ForceVisible`, `m_InnerSpotAngle`, `m_LightShadowCasterMode`, `m_LightUnit`, `m_LuxAtDistance`, `m_RenderingLayerMask`, `m_Shape`, `m_ShapeRadius`, `m_UseBoundingSphereOverride`, `m_UseColorTemperature`, `m_UseViewFrustumForShadowCasterCull` |
| `LightProbeGroup` | Behaviour | 220 | `m_GameObject`, `m_Enabled` |
| `LightProbeProxyVolume` | Behaviour | 259 | `m_BoundingBoxMode`, `m_BoundingBoxOrigin`, `m_BoundingBoxSize`, `m_Enabled`, `m_GameObject`, `m_ProbePositionMode`, `m_RefreshMode`, `m_ResolutionMode`, `m_ResolutionProbesPerUnit`, `m_ResolutionX`, `m_ResolutionY`, `m_ResolutionZ`, `m_DataFormat`, `m_QualityMode` |
| `MonoBehaviour` | Behaviour | 114 | `m_Enabled`, `m_GameObject`, `m_Name`, `m_Script` |
| `NavMeshAgent` | Behaviour | 195 | `m_Acceleration`, `m_AngularSpeed`, `m_AutoRepath`, `m_AutoTraverseOffMeshLink`, `m_BaseOffset`, `m_Enabled`, `m_GameObject`, `m_Height`, `m_ObstacleAvoidanceType`, `m_Radius`, `m_Speed`, `m_StoppingDistance`, `m_WalkableMask`, `avoidancePriority`, `m_AgentTypeID`, `m_AutoBraking` |
| `NavMeshObstacle` | Behaviour | 208 | `m_Enabled`, `m_GameObject`, `m_Carve`, `m_CarveOnlyStationary`, `m_Center`, `m_Extents`, `m_Height`, `m_MoveThreshold`, `m_Radius`, `m_Shape`, `m_TimeToStationary` |
| `NetworkView` | Behaviour | 148 | `m_Enabled`, `m_GameObject`, `m_Observed`, `m_StateSynchronization`, `m_ViewID` |
| `OffMeshLink` | Behaviour | 191 | `m_Activated`, `m_BiDirectional`, `m_CostOverride`, `m_End`, `m_GameObject`, `m_Start`, `m_AgentTypeID`, `m_AreaIndex`, `m_AutoUpdatePositions`, `m_DtPolyRef`, `m_Enabled`, `m_NavMeshLayer` |
| `ParticleSystemForceField` | Behaviour | 330 | `m_Enabled`, `m_GameObject`, `m_Parameters` |
| `PhysicsUpdateBehaviour2D` | Behaviour | 246 | `m_GameObject` |
| `ConstantForce2D` | PhysicsUpdateBehaviour2D | 247 | `m_Enabled`, `m_Force`, `m_GameObject`, `m_RelativeForce`, `m_Torque` |
| `PlayableDirector` | Behaviour | 320 | `m_DirectorUpdateMode`, `m_Enabled`, `m_ExposedReferences`, `m_GameObject`, `m_InitialState`, `m_InitialTime`, `m_PlayableAsset`, `m_SceneBindings`, `m_WrapMode` |
| `Projector` | Behaviour | 119 | `m_AspectRatio`, `m_Enabled`, `m_FarClipPlane`, `m_FieldOfView`, `m_GameObject`, `m_IgnoreLayers`, `m_Material`, `m_NearClipPlane`, `m_Orthographic`, `m_OrthographicSize` |
| `ReflectionProbe` | Behaviour | 215, 216 | `m_BackGroundColor`, `m_BakedTexture`, `m_BoxOffset`, `m_BoxProjection`, `m_BoxSize`, `m_ClearFlags`, `m_CullingMask`, `m_CustomBakedTexture`, `m_Enabled`, `m_FarClip`, `m_GameObject`, `m_HDR`, `m_Importance`, `m_IntensityMultiplier`, `m_Mode`, `m_NearClip`, `m_RefreshMode`, `m_RenderDynamicObjects`, `m_Resolution`, `m_ShadowDistance`, `m_TimeSlicingMode`, `m_Type`, `m_UpdateFrequency`, `m_UseOcclusionCulling`, `m_BlendDistance` |
| `Skybox` | Behaviour | 45 | `m_CustomSkybox`, `m_Enabled`, `m_GameObject` |
| `SortingGroup` | Behaviour | 210 | `m_Enabled`, `m_GameObject`, `m_SortingLayer`, `m_SortingOrder`, `m_Sort3DAs2D`, `m_SortAtRoot`, `m_SortingLayerID` |
| `StreamingController` | Behaviour | 1542919678 | `m_Enabled`, `m_GameObject`, `m_StreamingMipmapBias` |
| `Terrain` | Behaviour | 218 | `m_BakeLightProbesForTrees`, `m_ChunkDynamicUVST`, `m_DetailObjectDensity`, `m_DetailObjectDistance`, `m_DrawHeightmap`, `m_DrawTreesAndFoliage`, `m_DynamicUVST`, `m_Enabled`, `m_GameObject`, `m_HeightmapMaximumLOD`, `m_HeightmapPixelError`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MaterialTemplate`, `m_ReflectionProbeUsage`, `m_SplatMapDistance`, `m_TerrainData`, `m_TreeBillboardDistance`, `m_TreeCrossFadeLength`, `m_TreeDistance`, `m_TreeMaximumFullLODCount`, `m_AllowAutoConnect`, `m_CastShadows`, `m_DefaultSmoothness`, `m_DrawInstanced`, `m_EnableHeightmapRayTracing`, `m_EnableTreesAndDetailsRayTracing`, `m_ExplicitProbeSetHash`, `m_GroupingID`, `m_HeightmapMinimumLODSimplification`, `m_IgnoreQualitySettings`, `m_LegacyShininess`, `m_LegacySpecular`, `m_MaterialType`, `m_PreserveTreePrototypeLayers`, `m_RenderingLayerMask`, `m_ShadowCastingMode`, `m_StaticShadowCaster`, `m_TreeMotionVectorModeOverride`, `m_UseDefaultSmoothness` |
| `VideoPlayer` | Behaviour | 328 | `m_AspectRatio`, `m_AudioOutputMode`, `m_ControlledAudioTrackCount`, `m_DataSource`, `m_DirectAudioMutes`, `m_DirectAudioVolumes`, `m_Enabled`, `m_EnabledAudioTracks`, `m_FrameReadyEventEnabled`, `m_GameObject`, `m_Looping`, `m_PlayOnAwake`, `m_PlaybackSpeed`, `m_RenderMode`, `m_SkipOnDrop`, `m_TargetAudioSources`, `m_TargetCamera`, `m_TargetCameraAlpha`, `m_TargetMaterialProperty`, `m_TargetMaterialRenderer`, `m_TargetTexture`, `m_Url`, `m_VideoClip`, `m_WaitForFirstFrame`, `m_TargetCamera3DLayout`, `m_TargetMaterialName`, `m_TimeReference`, `m_TimeUpdateMode`, `m_VideoShaders` |
| `VisualEffect` | Behaviour | 2083052967 | `m_Asset`, `m_Enabled`, `m_GameObject`, `m_PropertySheet`, `m_ResetSeedOnPlay`, `m_StartSeed`, `m_AllowInstancing`, `m_InitialEventName`, `m_InitialEventNameOverriden`, `m_ReleaseInstanceOnDisable` |
| `WindZone` | Behaviour | 182 | `m_Enabled`, `m_GameObject`, `m_Mode`, `m_Radius`, `m_WindMain`, `m_WindPulseFrequency`, `m_WindPulseMagnitude`, `m_WindTurbulence` |
| `CanvasRenderer` | Component | 222 | `m_GameObject`, `m_CullTransparentMesh` |
| `Collider` | Component | 56 | `m_GameObject`, `m_MaxLimitX`, `m_MaxLimitY`, `m_MaxLimitZ`, `m_MinLimitX`, `m_Type`, `m_X`, `m_XMotionType`, `m_YMotionType`, `m_ZMotionType` |
| `BoxCollider` | Collider | 65 | `m_Center`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Size`, `m_ExcludeLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_ProvidesContacts` |
| `CapsuleCollider` | Collider | 136 | `m_Center`, `m_Direction`, `m_Enabled`, `m_GameObject`, `m_Height`, `m_IsTrigger`, `m_Material`, `m_Radius`, `m_ExcludeLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_ProvidesContacts` |
| `CharacterController` | Collider | 143 | `m_Center`, `m_GameObject`, `m_Height`, `m_MinMoveDistance`, `m_Radius`, `m_SkinWidth`, `m_SlopeLimit`, `m_StepOffset`, `m_Enabled`, `m_ExcludeLayers`, `m_IncludeLayers`, `m_IsTrigger`, `m_LayerOverridePriority`, `m_Material`, `m_ProvidesContacts` |
| `MeshCollider` | Collider | 64 | `m_Convex`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Mesh`, `m_CookingOptions`, `m_ExcludeLayers`, `m_IncludeLayers`, `m_InflateMesh`, `m_LayerOverridePriority`, `m_ProvidesContacts`, `m_SkinWidth`, `m_SmoothSphereCollisions` |
| `RaycastCollider` | Collider | 140 | `m_Center`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Length`, `m_Material` |
| `SphereCollider` | Collider | 135 | `m_Center`, `m_Enabled`, `m_GameObject`, `m_IsTrigger`, `m_Material`, `m_Radius`, `m_ExcludeLayers`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_ProvidesContacts` |
| `TerrainCollider` | Collider | 154 | `m_Enabled`, `m_GameObject`, `m_TerrainData`, `m_CreateTreeColliders`, `m_EnableTreeColliders`, `m_ExcludeLayers`, `m_IncludeLayers`, `m_IsTrigger`, `m_LayerOverridePriority`, `m_Material`, `m_ProvidesContacts` |
| `WheelCollider` | Collider | 146 | `m_Center`, `m_ForwardFriction`, `m_GameObject`, `m_Mass`, `m_Radius`, `m_SidewaysFriction`, `m_SuspensionDistance`, `m_SuspensionSpring`, `m_Enabled`, `m_ExcludeLayers`, `m_ForceAppPointDistance`, `m_IncludeLayers`, `m_LayerOverridePriority`, `m_ProvidesContacts`, `m_WheelDampingRate` |
| `FakeComponent` | Component | 1803986026 | `m_GameObject` |
| `Joint` | Component | 57 | `m_GameObject` |
| `CharacterJoint` | Joint | 144 | `m_Anchor`, `m_Axis`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedBody`, `m_GameObject`, `m_HighTwistLimit`, `m_LowTwistLimit`, `m_Swing1Limit`, `m_Swing2Limit`, `m_SwingAxis`, `m_AutoConfigureConnectedAnchor`, `m_ConnectedAnchor`, `m_ConnectedArticulationBody`, `m_ConnectedMassScale`, `m_EnableCollision`, `m_EnablePreprocessing`, `m_EnableProjection`, `m_Enabled`, `m_MassScale`, `m_ProjectionAngle`, `m_ProjectionDistance`, `m_SwingLimitSpring`, `m_TwistLimitSpring` |
| `ConfigurableJoint` | Joint | 153 | `m_Anchor`, `m_AngularXDrive`, `m_AngularXMotion`, `m_AngularYLimit`, `m_AngularYMotion`, `m_AngularYZDrive`, `m_AngularZLimit`, `m_AngularZMotion`, `m_Axis`, `m_BreakForce`, `m_BreakTorque`, `m_ConfiguredInWorldSpace`, `m_ConnectedBody`, `m_GameObject`, `m_HighAngularXLimit`, `m_LinearLimit`, `m_LowAngularXLimit`, `m_ProjectionAngle`, `m_ProjectionDistance`, `m_ProjectionMode`, `m_RotationDriveMode`, `m_SecondaryAxis`, `m_SlerpDrive`, `m_TargetAngularVelocity`, `m_TargetPosition`, `m_TargetRotation`, `m_TargetVelocity`, `m_XDrive`, `m_XMotion`, `m_YDrive`, `m_YMotion`, `m_ZDrive`, `m_ZMotion`, `m_AngularXLimitSpring`, `m_AngularYZLimitSpring`, `m_AutoConfigureConnectedAnchor`, `m_ConnectedAnchor`, `m_ConnectedArticulationBody`, `m_ConnectedMassScale`, `m_EnableCollision`, `m_EnablePreprocessing`, `m_Enabled`, `m_LinearLimitSpring`, `m_MassScale`, `m_SwapBodies` |
| `FixedJoint` | Joint | 138 | `m_BreakForce`, `m_BreakTorque`, `m_ConnectedBody`, `m_GameObject`, `m_ConnectedArticulationBody`, `m_ConnectedMassScale`, `m_EnableCollision`, `m_EnablePreprocessing`, `m_Enabled`, `m_MassScale` |
| `HingeJoint` | Joint | 59 | `m_Anchor`, `m_Axis`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedBody`, `m_GameObject`, `m_Limits`, `m_Motor`, `m_Spring`, `m_UseLimits`, `m_UseMotor`, `m_UseSpring`, `m_AutoConfigureConnectedAnchor`, `m_ConnectedAnchor`, `m_ConnectedArticulationBody`, `m_ConnectedMassScale`, `m_EnableCollision`, `m_EnablePreprocessing`, `m_Enabled`, `m_ExtendedLimits`, `m_MassScale`, `m_UseAcceleration` |
| `SpringJoint` | Joint | 145 | `m_Anchor`, `m_BreakForce`, `m_BreakTorque`, `m_ConnectedBody`, `m_Damper`, `m_GameObject`, `m_MaxDistance`, `m_MinDistance`, `m_Spring`, `m_AutoConfigureConnectedAnchor`, `m_Axis`, `m_ConnectedAnchor`, `m_ConnectedArticulationBody`, `m_ConnectedMassScale`, `m_EnableCollision`, `m_EnablePreprocessing`, `m_Enabled`, `m_MassScale`, `m_Tolerance` |
| `LODGroup` | Component | 205 | `m_Enabled`, `m_GameObject`, `m_LODs`, `m_LocalReferencePoint`, `m_Size`, `m_AnimateCrossFading`, `m_FadeMode`, `m_GlobalIlluminationLOD`, `m_LastLODIsBillboard`, `m_ScreenRelativeTransitionHeight` |
| `MeshFilter` | Component | 33 | `m_GameObject`, `m_Mesh` |
| `MultiplayerRolesData` | Component | — | `m_ComponentsRolesMasks`, `m_GameObject`, `m_GameObjectRolesMask` |
| `OcclusionArea` | Component | 192 | `m_Center`, `m_GameObject`, `m_IsViewVolume`, `m_Size`, `m_IsTargetVolume`, `m_TargetResolution` |
| `OcclusionPortal` | Component | 41 | `m_Center`, `m_GameObject`, `m_Open`, `m_Size` |
| `ParticleAnimator` | Component | 12 | `Does_Animate_Color`, `autodestruct`, `colorAnimation_0_`, `colorAnimation_1_`, `colorAnimation_2_`, `colorAnimation_3_`, `colorAnimation_4_`, `damping`, `force`, `localRotationAxis`, `m_GameObject`, `rndForce`, `sizeGrow`, `stopSimulation`, `worldRotationAxis` |
| `ParticleEmitter` | Component | 88 | `m_GameObject` |
| `EllipsoidParticleEmitter` | ParticleEmitter | 15 | `Simulate_in_Worldspace`, `angularVelocity`, `emitterVelocityScale`, `localVelocity`, `m_Ellipsoid`, `m_Emit`, `m_Enabled`, `m_GameObject`, `m_MinEmitterRange`, `m_OneShot`, `maxEmission`, `maxEnergy`, `maxSize`, `minEmission`, `minEnergy`, `minSize`, `rndAngularVelocity`, `rndRotation`, `rndVelocity`, `tangentVelocity`, `worldVelocity` |
| `MeshParticleEmitter` | ParticleEmitter | 87 | `Simulate_in_Worldspace`, `angularVelocity`, `emitterVelocityScale`, `localVelocity`, `m_Emit`, `m_Enabled`, `m_GameObject`, `m_InterpolateTriangles`, `m_MaxNormalVelocity`, `m_Mesh`, `m_MinNormalVelocity`, `m_OneShot`, `m_Systematic`, `maxEmission`, `maxEnergy`, `maxSize`, `minEmission`, `minEnergy`, `minSize`, `rndAngularVelocity`, `rndRotation`, `rndVelocity`, `tangentVelocity`, `worldVelocity` |
| `ParticleSystem` | Component | 198 | `ClampVelocityModule`, `CollisionModule`, `ColorBySpeedModule`, `ColorModule`, `EmissionModule`, `ForceModule`, `InitialModule`, `RotationBySpeedModule`, `RotationModule`, `ShapeModule`, `SizeBySpeedModule`, `SizeModule`, `SubModule`, `UVModule`, `VelocityModule`, `lengthInSec`, `looping`, `m_GameObject`, `moveWithTransform`, `playOnAwake`, `prewarm`, `randomSeed`, `startDelay`, `CustomDataModule`, `ExternalForcesModule`, `InheritVelocityModule`, `LifetimeByEmitterSpeedModule`, `LightsModule`, `NoiseModule`, `TrailModule`, `TriggerModule`, `autoRandomSeed`, `cullingMode`, `emitterVelocityMode`, `moveWithCustomTransform`, `ringBufferLoopRange`, `ringBufferMode`, `scalingMode`, `simulationSpeed`, `speed`, `stopAction`, `useRigidbodyForVelocity`, `useUnscaledTime` |
| `Pipeline` | Component | 17 | `m_GameObject` |
| `Renderer` | Component | 25 | `m_GameObject` |
| `BillboardRenderer` | Renderer | 227 | `m_Billboard`, `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_Materials`, `m_ProbeAnchor`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_SortingOrder`, `m_StaticBatchRoot`, `m_DynamicOccludee`, `m_ForceMeshLod`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_MaskInteraction`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_UseLightProbes` |
| `ClothRenderer` | Renderer | 161 | `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_PauseWhenNotVisible`, `m_ReceiveShadows`, `m_StaticBatchRoot`, `m_SubsetIndices`, `m_LightProbeAnchor`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_UseLightProbes` |
| `LineRenderer` | Renderer | 120 | `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_Parameters`, `m_Positions`, `m_ReceiveShadows`, `m_StaticBatchRoot`, `m_UseWorldSpace`, `m_ApplyActiveColorSpace`, `m_DynamicOccludee`, `m_ForceMeshLod`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_Loop`, `m_MaskInteraction`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_UseLightProbes` |
| `RendererFake` | LineRenderer | 646504946 | `m_CastShadows`, `m_DynamicOccludee`, `m_Enabled`, `m_GameObject`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_Loop`, `m_Materials`, `m_MotionVectors`, `m_Parameters`, `m_Positions`, `m_ProbeAnchor`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_UseWorldSpace`, `m_RayTraceProcedural`, `m_RayTracingMode` |
| `MeshRenderer` | Renderer | 23 | `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_ReceiveShadows`, `m_StaticBatchRoot`, `m_AdditionalVertexStreams`, `m_DynamicOccludee`, `m_EnlightenVertexStream`, `m_ForceMeshLod`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_UseLightProbes` |
| `PanelRenderer` | Renderer | — | `m_CastShadows`, `m_DynamicOccludee`, `m_Enabled`, `m_ForceMeshLod`, `m_GameObject`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_Materials`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_PanelSettings`, `m_ParentUI`, `m_Pivot`, `m_PivotReferenceSize`, `m_Position`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_StaticShadowCaster`, `m_WorldSpaceHeight`, `m_WorldSpaceSizeMode`, `m_WorldSpaceWidth`, `sourceAsset` |
| `ParticleRenderer` | Renderer | 26 | `UV_Animation`, `m_CameraVelocityScale`, `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LengthScale`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_MaxParticleSize`, `m_ReceiveShadows`, `m_StaticBatchRoot`, `m_StretchParticles`, `m_VelocityScale`, `m_DynamicOccludee`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_MotionVectors`, `m_ProbeAnchor`, `m_ReflectionProbeUsage`, `m_RenderingLayerMask`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_SubsetIndices`, `m_UseLightProbes` |
| `ParticleSystemRenderer` | Renderer | 199 | `m_CameraVelocityScale`, `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LengthScale`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_MaxParticleSize`, `m_Mesh`, `m_ReceiveShadows`, `m_RenderMode`, `m_SortMode`, `m_SortingFudge`, `m_StaticBatchRoot`, `m_VelocityScale`, `m_AllowRoll`, `m_ApplyActiveColorSpace`, `m_DynamicOccludee`, `m_EnableGPUInstancing`, `m_Flip`, `m_ForceMeshLod`, `m_FreeformStretching`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_Mesh1`, `m_Mesh2`, `m_Mesh3`, `m_MeshDistribution`, `m_MeshLodSelectionBias`, `m_MeshWeighting`, `m_MeshWeighting1`, `m_MeshWeighting2`, `m_MeshWeighting3`, `m_MinParticleSize`, `m_MotionVectors`, `m_NormalDirection`, `m_Pivot`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReflectionProbeUsage`, `m_RenderAlignment`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_RotateWithStretchDirection`, `m_ShadowBias`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_TrailVertexStreams`, `m_UseCustomTrailVertexStreams`, `m_UseCustomVertexStreams`, `m_UseLightProbes`, `m_VertexStreamMask`, `m_VertexStreams` |
| `RenderAs2D` | Renderer | — | `m_CastShadows`, `m_DynamicOccludee`, `m_Enabled`, `m_ForceMeshLod`, `m_GameObject`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_Materials`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_StaticShadowCaster`, `m_Owner`, `m_OwningSortingGroup` |
| `SkinnedMeshRenderer` | Renderer | 137 | `m_AABB`, `m_Bones`, `m_CastShadows`, `m_DirtyAABB`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_Mesh`, `m_Quality`, `m_ReceiveShadows`, `m_StaticBatchRoot`, `m_UpdateWhenOffscreen`, `m_BlendShapeWeights`, `m_DynamicOccludee`, `m_ForceMeshLod`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_RootBone`, `m_SkinnedMotionVectors`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_UseLightProbes` |
| `SpriteMask` | Renderer | 331 | `m_BackSortingLayer`, `m_BackSortingOrder`, `m_CastShadows`, `m_Enabled`, `m_FrontSortingLayer`, `m_FrontSortingOrder`, `m_GameObject`, `m_IsCustomRangeActive`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MaskAlphaCutoff`, `m_Materials`, `m_MotionVectors`, `m_ProbeAnchor`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_Sprite`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_BackSortingLayerID`, `m_DynamicOccludee`, `m_ForceMeshLod`, `m_FrontSortingLayerID`, `m_MaskInteraction`, `m_MaskSource`, `m_MeshLodSelectionBias`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SpriteSortPoint`, `m_StaticShadowCaster` |
| `SpriteRenderer` | Renderer | 212 | `m_CastShadows`, `m_Color`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_ReceiveShadows`, `m_SortingOrder`, `m_Sprite`, `m_StaticBatchRoot`, `m_AdaptiveModeThreshold`, `m_DrawMode`, `m_DynamicOccludee`, `m_FlipX`, `m_FlipY`, `m_ForceMeshLod`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_Size`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SpriteSortPoint`, `m_SpriteTileMode`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_UseLightProbes`, `m_WasSpriteAssigned` |
| `SpriteShapeRenderer` | Renderer | 1971053207 | `m_CastShadows`, `m_Color`, `m_DynamicOccludee`, `m_Enabled`, `m_GameObject`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_LocalAABB`, `m_MaskInteraction`, `m_Materials`, `m_MotionVectors`, `m_ProbeAnchor`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_RenderingLayerMask`, `m_ShapeTexture`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_Sprites`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_ForceMeshLod`, `m_MeshLodSelectionBias`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_RendererPriority`, `m_SmallMeshCulling`, `m_SpriteSortPoint`, `m_StaticShadowCaster` |
| `TilemapRenderer` | Renderer | 483693784 | `m_CastShadows`, `m_ChunkSize`, `m_DynamicOccludee`, `m_Enabled`, `m_GameObject`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_Materials`, `m_MaxChunkCount`, `m_MaxFrameAge`, `m_MotionVectors`, `m_ProbeAnchor`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_SortOrder`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_ChunkCullingBounds`, `m_DetectChunkCullingBounds`, `m_ForceMeshLod`, `m_MeshLodSelectionBias`, `m_Mode`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_StaticShadowCaster` |
| `TrailRenderer` | Renderer | 96 | `m_Autodestruct`, `m_CastShadows`, `m_Enabled`, `m_GameObject`, `m_LightmapIndex`, `m_LightmapTilingOffset`, `m_Materials`, `m_MinVertexDistance`, `m_ReceiveShadows`, `m_StaticBatchRoot`, `m_Time`, `m_ApplyActiveColorSpace`, `m_Colors`, `m_DynamicOccludee`, `m_Emitting`, `m_EndWidth`, `m_ForceMeshLod`, `m_LightProbeAnchor`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_Parameters`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StartWidth`, `m_StaticBatchInfo`, `m_StaticShadowCaster`, `m_SubsetIndices`, `m_UseLightProbes` |
| `UIRenderer` | Renderer | — | `m_GameObject`, `m_CastShadows`, `m_DynamicOccludee`, `m_Enabled`, `m_ForceMeshLod`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MaskInteraction`, `m_Materials`, `m_MeshLodSelectionBias`, `m_MotionVectors`, `m_ProbeAnchor`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SmallMeshCulling`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_StaticShadowCaster` |
| `VFXRenderer` | Renderer | 73398921 | `m_CastShadows`, `m_DynamicOccludee`, `m_Enabled`, `m_GameObject`, `m_LightProbeUsage`, `m_LightProbeVolumeOverride`, `m_LightmapIndex`, `m_LightmapIndexDynamic`, `m_LightmapTilingOffset`, `m_LightmapTilingOffsetDynamic`, `m_MotionVectors`, `m_ProbeAnchor`, `m_ReceiveShadows`, `m_ReflectionProbeUsage`, `m_RendererPriority`, `m_RenderingLayerMask`, `m_SortingLayer`, `m_SortingLayerID`, `m_SortingOrder`, `m_StaticBatchInfo`, `m_StaticBatchRoot`, `m_ForceMeshLod`, `m_MaskInteraction`, `m_Materials`, `m_MeshLodSelectionBias`, `m_RayTraceProcedural`, `m_RayTracingAccelStructBuildFlags`, `m_RayTracingAccelStructBuildFlagsOverride`, `m_RayTracingMode`, `m_SmallMeshCulling`, `m_StaticShadowCaster` |
| `Rigidbody` | Component | 54 | `m_CollisionDetection`, `m_Constraints`, `m_GameObject`, `m_Interpolate`, `m_IsKinematic`, `m_Mass`, `m_UseGravity`, `m_AngularDamping`, `m_AngularDrag`, `m_CenterOfMass`, `m_Drag`, `m_ExcludeLayers`, `m_ImplicitCom`, `m_ImplicitTensor`, `m_IncludeLayers`, `m_InertiaRotation`, `m_InertiaTensor`, `m_LinearDamping` |
| `Rigidbody2D` | Component | 50 | `m_CollisionDetection`, `m_GameObject`, `m_GravityScale`, `m_Interpolate`, `m_Mass`, `m_SleepingMode`, `m_AngularDamping`, `m_AngularDrag`, `m_BodyType`, `m_Constraints`, `m_ExcludeLayers`, `m_FixedAngle`, `m_IncludeLayers`, `m_IsKinematic`, `m_LinearDamping`, `m_LinearDrag`, `m_Material`, `m_Simulated`, `m_UseAutoMass`, `m_UseFullKinematicContacts` |
| `TextMesh` | Component | 102 | `m_Alignment`, `m_Anchor`, `m_CharacterSize`, `m_Font`, `m_FontSize`, `m_FontStyle`, `m_GameObject`, `m_LineSpacing`, `m_OffsetZ`, `m_TabSize`, `m_Text`, `m_Color`, `m_RichText` |
| `Transform` | Component | 4 | `m_Children`, `m_Father`, `m_GameObject`, `m_LocalPosition`, `m_LocalRotation`, `m_LocalScale` |
| `RectTransform` | Transform | 224 | `m_AnchorMax`, `m_AnchorMin`, `m_GameObject`, `m_Pivot`, `m_SizeDelta`, `m_AnchoredPosition`, `m_Children`, `m_Father`, `m_LocalPosition`, `m_LocalRotation`, `m_LocalScale`, `m_Position` |
| `Tree` | Component | 193 | `m_GameObject`, `m_SpeedTreeWindAsset` |
| `WorldAnchor` | Component | 362 | `m_GameObject` |
| `WorldParticleCollider` | Component | 76 | `m_BounceFactor`, `m_CollidesWith`, `m_CollisionEnergyLoss`, `m_GameObject`, `m_MinKillVelocity`, `m_SendCollisionMessage` |
| `GameObject` | EditorExtension | 1 | `m_Component`, `m_IsActive`, `m_Layer`, `m_Name`, `m_Tag` |
| `NamedObject` | EditorExtension, ABC | 130 | — |
| `AnimatorState` | NamedObject | 1102 | `m_CycleOffset`, `m_IKOnFeet`, `m_Mirror`, `m_Motion`, `m_Name`, `m_Position`, `m_Speed`, `m_StateMachineBehaviours`, `m_Tag`, `m_Transitions`, `m_WriteDefaultValues`, `m_CycleOffsetParameter`, `m_CycleOffsetParameterActive`, `m_MirrorParameter`, `m_MirrorParameterActive`, `m_SpeedParameter`, `m_SpeedParameterActive`, `m_TimeParameter`, `m_TimeParameterActive` |
| `AnimatorStateMachine` | NamedObject | 1107 | `m_AnyStatePosition`, `m_AnyStateTransitions`, `m_ChildStateMachines`, `m_ChildStates`, `m_DefaultState`, `m_EntryPosition`, `m_EntryTransitions`, `m_ExitPosition`, `m_Name`, `m_ParentStateMachinePosition`, `m_StateMachineBehaviours`, `m_StateMachineTransitions` |
| `AnimatorTransitionBase` | NamedObject | 1111 | `m_Conditions`, `m_DstState`, `m_DstStateMachine`, `m_IsExit`, `m_Mute`, `m_Name`, `m_Solo` |
| `AnimatorStateTransition` | AnimatorTransitionBase | 1101 | `m_CanTransitionToSelf`, `m_Conditions`, `m_DstState`, `m_DstStateMachine`, `m_ExitTime`, `m_HasExitTime`, `m_InterruptionSource`, `m_IsExit`, `m_Mute`, `m_Name`, `m_OrderedInterruption`, `m_Solo`, `m_TransitionDuration`, `m_TransitionOffset`, `m_HasFixedDuration` |
| `AnimatorTransition` | AnimatorTransitionBase | 1109 | `m_Conditions`, `m_DstState`, `m_DstStateMachine`, `m_IsExit`, `m_Mute`, `m_Name`, `m_Solo` |
| `AssetBundle` | NamedObject | 142 | `m_Container`, `m_MainAsset`, `m_Name`, `m_PreloadTable`, `m_AssetBundleName`, `m_ClassCompatibility`, `m_ClassVersionMap`, `m_Dependencies`, `m_ExplicitDataLayout`, `m_IsStreamedSceneAssetBundle`, `m_PathFlags`, `m_RuntimeCompatibility`, `m_SceneHashes`, `m_ScriptCompatibility` |
| `AssetBundleManifest` | NamedObject | 290 | `AssetBundleInfos`, `AssetBundleNames`, `AssetBundlesWithVariant`, `m_Name` |
| `AssetImportInProgressProxy` | NamedObject | 369655926 | `m_Name` |
| `AssetImporter` | NamedObject, ABC | 1003 | — |
| `ASTCImporter` | AssetImporter | 1053 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_Name`, `m_UserData` |
| `AndroidAssetPackImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `AssemblyDefinitionImporter` | AssetImporter | 1766753193 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UserData`, `m_UsedFileIDs` |
| `AssemblyDefinitionReferenceImporter` | AssetImporter | 294290339 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `AudioImporter` | AssetImporter | 1020 | `m_3D`, `m_ForceToMono`, `m_Name`, `audio_preview_data`, `m_Ambisonic`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_DefaultSettings`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_Format`, `m_LoadInBackground`, `m_Loopable`, `m_NewHashIdentity`, `m_Normalize`, `m_OldHashIdentity`, `m_Output`, `m_PlatformSettingOverrides`, `m_PreloadAudioData`, `m_PreviewData`, `m_PreviewDataLength`, `m_Quality`, `m_Stream`, `m_UseHardware`, `m_UsedFileIDs`, `m_UserData` |
| `BlockShaderImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_EmbedBlocksInGeneratedShader`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `BuildArchiveImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `BuildInstructionImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `BuildMetaDataImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `C4DImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `ComputeShaderImporter` | AssetImporter | 1008 | `m_Name`, `m_UserData`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_CurrentAPIMask`, `m_CurrentBuildTarget`, `m_ExternalObjects`, `m_PreprocessorOverride`, `m_UsedFileIDs` |
| `DDSImporter` | AssetImporter | 1046 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_FileIDToRecycleName`, `m_IsReadable`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UserData` |
| `DefaultImporter` | AssetImporter | 1030 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `IHVImageFormatImporter` | AssetImporter | 1055 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_IsReadable`, `m_Name`, `m_TextureSettings`, `m_UserData`, `m_ExternalObjects`, `m_IgnoreMipmapLimit`, `m_MipmapLimitGroupName`, `m_StreamingMipmaps`, `m_StreamingMipmapsPriority`, `m_UsedFileIDs`, `m_sRGBTexture` |
| `KTXImporter` | AssetImporter | 1054 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_Name`, `m_UserData` |
| `LibraryAssetImporter` | AssetImporter | 1038 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `LocalizationImporter` | AssetImporter | 1027052791 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UserData`, `m_UsedFileIDs` |
| `ModelImporter` | AssetImporter, ABC | 1040 | — |
| `FBXImporter` | ModelImporter | 1041 | `m_AddColliders`, `m_AnimationCompression`, `m_AnimationPositionError`, `m_AnimationRotationError`, `m_AnimationScaleError`, `m_AnimationWrapMode`, `m_BakeSimulation`, `m_ClipAnimations`, `m_GlobalScale`, `m_HasExtraRoot`, `m_ImportedRoots`, `m_MeshCompression`, `m_Name`, `m_UseFileUnits`, `normalSmoothAngle`, `bakeAxisConversion`, `blendShapeNormalImportMode`, `calculateBlendshapeNormalsDeltaFromImportedNormals`, `generateMeshLods`, `generateSecondaryUV`, `indexFormat`, `keepQuads`, `legacyComputeAllNormalsFromSmoothingGroupsWhenMeshHasBlendShapes`, `m_AddHumanoidExtraRootOnlyWhenUsingAvatar`, `m_AdditionalBone`, `m_AnimationDoRetargetingWarnings`, `m_AnimationImportErrors`, `m_AnimationImportWarnings`, `m_AnimationRetargetingWarnings`, `m_AnimationType`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_AutoGenerateAvatarMappingIfUnspecified`, `m_AutoMapExternalMaterials`, `m_AvatarSetup`, `m_ContainsAnimation`, `m_CopyAvatar`, `m_ExternalObjects`, `m_ExtraExposedTransformPaths`, `m_ExtraUserProperties`, `m_FileIDToRecycleName`, `m_FileIdsGeneration`, `m_FileScale`, `m_FileScaleFactor`, `m_FileScaleUnit`, `m_FirstImportVersion`, `m_GenerateAnimations`, `m_GenerateMaterials`, `m_HasEmbeddedTextures`, `m_HasPreviousCalculatedGlobalScale`, `m_HumanDescription`, `m_HumanoidOversampling`, `m_ImportAnimatedCustomProperties`, `m_ImportAnimation`, `m_ImportBlendShapeDeformPercent`, `m_ImportBlendShapes`, `m_ImportCameras`, `m_ImportConstraints`, `m_ImportLights`, `m_ImportMaterials`, `m_ImportPhysicalCameras`, `m_ImportVisibility`, `m_ImportedTakeInfos`, `m_InternalIDToNameTable`, `m_IsReadable`, `m_LODScreenPercentages`, `m_LastHumanDescriptionAvatarSource`, `m_LegacyGenerateAnimations`, `m_MaterialImportMode`, `m_MaterialLocation`, `m_MaterialName`, `m_MaterialSearch`, `m_Materials`, `m_MeshSettings_generateSecondaryUV`, `m_MeshSettings_normalImportMode`, `m_MeshSettings_secondaryUVAngleDistortion`, `m_MeshSettings_secondaryUVAreaDistortion`, `m_MeshSettings_secondaryUVHardAngle`, `m_MeshSettings_secondaryUVPackMargin`, `m_MeshSettings_swapUVChannels`, `m_MeshSettings_tangentImportMode`, `m_MotionNodeName`, `m_NewHashIdentity`, `m_NodeNameCollisionStrategy`, `m_OldHashIdentity`, `m_OptimizeGameObjects`, `m_PreserveHierarchy`, `m_PreviousCalculatedGlobalScale`, `m_ReferencedClips`, `m_RemapMaterialsIfMaterialImportModeIsNone`, `m_RemoveConstantScaleCurves`, `m_ResampleCurves`, `m_ResampleRotations`, `m_RigImportErrors`, `m_RigImportWarnings`, `m_SortHierarchyByName`, `m_SplitAnimations`, `m_StrictVertexDataChecks`, `m_SupportsEmbeddedMaterials`, `m_UseFileScale`, `m_UseSRGBMaterialColor`, `m_UsedFileIDs`, `m_UserData`, `maxBonesPerVertex`, `maximumMeshLod`, `meshLodGenerationFlags`, `meshOptimizationFlags`, `minBoneWeight`, `normalCalculationMode`, `normalImportMode`, `normalSmoothingSource`, `optimizeBones`, `optimizeMesh`, `optimizeMeshForGPU`, `secondaryUVAngleDistortion`, `secondaryUVAreaDistortion`, `secondaryUVHardAngle`, `secondaryUVMarginMethod`, `secondaryUVMinLightmapResolution`, `secondaryUVMinObjectScale`, `secondaryUVPackMargin`, `skinWeightsMode`, `splitTangentsAcrossUV`, `swapUVChannels`, `tangentImportMode`, `weldVertices` |
| `Mesh3DSImporter` | ModelImporter | 1005 | `m_AddColliders`, `m_AnimationCompression`, `m_AnimationPositionError`, `m_AnimationRotationError`, `m_AnimationScaleError`, `m_AnimationWrapMode`, `m_BakeSimulation`, `m_ClipAnimations`, `m_GlobalScale`, `m_HasExtraRoot`, `m_ImportedRoots`, `m_MeshCompression`, `m_Name`, `m_UseFileUnits`, `normalSmoothAngle`, `bakeAxisConversion`, `blendShapeNormalImportMode`, `calculateBlendshapeNormalsDeltaFromImportedNormals`, `generateMeshLods`, `generateSecondaryUV`, `indexFormat`, `keepQuads`, `legacyComputeAllNormalsFromSmoothingGroupsWhenMeshHasBlendShapes`, `m_AddHumanoidExtraRootOnlyWhenUsingAvatar`, `m_AdditionalBone`, `m_AnimationDoRetargetingWarnings`, `m_AnimationImportErrors`, `m_AnimationImportWarnings`, `m_AnimationRetargetingWarnings`, `m_AnimationType`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_AutoGenerateAvatarMappingIfUnspecified`, `m_AutoMapExternalMaterials`, `m_AvatarSetup`, `m_ContainsAnimation`, `m_CopyAvatar`, `m_ExternalObjects`, `m_ExtraExposedTransformPaths`, `m_ExtraUserProperties`, `m_FileIDToRecycleName`, `m_FileIdsGeneration`, `m_FileScale`, `m_FileScaleFactor`, `m_FileScaleUnit`, `m_FirstImportVersion`, `m_GenerateAnimations`, `m_GenerateMaterials`, `m_HasEmbeddedTextures`, `m_HasPreviousCalculatedGlobalScale`, `m_HumanDescription`, `m_HumanoidOversampling`, `m_ImportAnimatedCustomProperties`, `m_ImportAnimation`, `m_ImportBlendShapeDeformPercent`, `m_ImportBlendShapes`, `m_ImportCameras`, `m_ImportConstraints`, `m_ImportLights`, `m_ImportMaterials`, `m_ImportPhysicalCameras`, `m_ImportVisibility`, `m_ImportedTakeInfos`, `m_InternalIDToNameTable`, `m_IsReadable`, `m_LODScreenPercentages`, `m_LastHumanDescriptionAvatarSource`, `m_LegacyGenerateAnimations`, `m_MaterialImportMode`, `m_MaterialLocation`, `m_MaterialName`, `m_MaterialSearch`, `m_Materials`, `m_MeshSettings_generateSecondaryUV`, `m_MeshSettings_normalImportMode`, `m_MeshSettings_secondaryUVAngleDistortion`, `m_MeshSettings_secondaryUVAreaDistortion`, `m_MeshSettings_secondaryUVHardAngle`, `m_MeshSettings_secondaryUVPackMargin`, `m_MeshSettings_swapUVChannels`, `m_MeshSettings_tangentImportMode`, `m_MotionNodeName`, `m_NewHashIdentity`, `m_NodeNameCollisionStrategy`, `m_OldHashIdentity`, `m_OptimizeGameObjects`, `m_PreserveHierarchy`, `m_PreviousCalculatedGlobalScale`, `m_ReferencedClips`, `m_RemapMaterialsIfMaterialImportModeIsNone`, `m_RemoveConstantScaleCurves`, `m_ResampleCurves`, `m_ResampleRotations`, `m_RigImportErrors`, `m_RigImportWarnings`, `m_SortHierarchyByName`, `m_SplitAnimations`, `m_StrictVertexDataChecks`, `m_SupportsEmbeddedMaterials`, `m_UseFileScale`, `m_UseSRGBMaterialColor`, `m_UsedFileIDs`, `m_UserData`, `maxBonesPerVertex`, `maximumMeshLod`, `meshLodGenerationFlags`, `meshOptimizationFlags`, `minBoneWeight`, `normalCalculationMode`, `normalImportMode`, `normalSmoothingSource`, `optimizeBones`, `optimizeMesh`, `optimizeMeshForGPU`, `secondaryUVAngleDistortion`, `secondaryUVAreaDistortion`, `secondaryUVHardAngle`, `secondaryUVMarginMethod`, `secondaryUVMinLightmapResolution`, `secondaryUVMinObjectScale`, `secondaryUVPackMargin`, `skinWeightsMode`, `splitTangentsAcrossUV`, `swapUVChannels`, `tangentImportMode`, `weldVertices` |
| `SketchUpImporter` | ModelImporter | 1124 | `generateSecondaryUV`, `keepQuads`, `m_AddColliders`, `m_AdditionalBone`, `m_AnimationCompression`, `m_AnimationDoRetargetingWarnings`, `m_AnimationImportErrors`, `m_AnimationImportWarnings`, `m_AnimationPositionError`, `m_AnimationRetargetingWarnings`, `m_AnimationRotationError`, `m_AnimationScaleError`, `m_AnimationType`, `m_AnimationWrapMode`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_AssetHash`, `m_BakeSimulation`, `m_ClipAnimations`, `m_ExtraExposedTransformPaths`, `m_FileScale`, `m_FileUnit`, `m_GenerateBackFace`, `m_GlobalScale`, `m_HasExtraRoot`, `m_HumanDescription`, `m_ImportAnimation`, `m_ImportBlendShapes`, `m_ImportedRoots`, `m_ImportedTakeInfos`, `m_IsReadable`, `m_LODScreenPercentages`, `m_LastHumanDescriptionAvatarSource`, `m_Latitude`, `m_LegacyGenerateAnimations`, `m_Longitude`, `m_MaterialName`, `m_MaterialSearch`, `m_MergeCoplanarFaces`, `m_MeshCompression`, `m_MotionNodeName`, `m_Name`, `m_NorthCorrection`, `m_OptimizeGameObjects`, `m_ReferencedClips`, `m_SelectedNodes`, `m_SketchUpImportData`, `m_UseFileScale`, `m_UseFileUnits`, `m_UserData`, `normalImportMode`, `normalSmoothAngle`, `secondaryUVAngleDistortion`, `secondaryUVAreaDistortion`, `secondaryUVHardAngle`, `secondaryUVPackMargin`, `swapUVChannels`, `tangentImportMode`, `weldVertices`, `bakeAxisConversion`, `blendShapeNormalImportMode`, `calculateBlendshapeNormalsDeltaFromImportedNormals`, `generateMeshLods`, `indexFormat`, `legacyComputeAllNormalsFromSmoothingGroupsWhenMeshHasBlendShapes`, `m_AddHumanoidExtraRootOnlyWhenUsingAvatar`, `m_AutoGenerateAvatarMappingIfUnspecified`, `m_AutoMapExternalMaterials`, `m_AvatarSetup`, `m_ContainsAnimation`, `m_CopyAvatar`, `m_ExternalObjects`, `m_ExtraUserProperties`, `m_FileIDToRecycleName`, `m_FileIdsGeneration`, `m_FileScaleFactor`, `m_FileScaleUnit`, `m_HasEmbeddedTextures`, `m_HasPreviousCalculatedGlobalScale`, `m_HumanoidOversampling`, `m_ImportAnimatedCustomProperties`, `m_ImportBlendShapeDeformPercent`, `m_ImportCameras`, `m_ImportConstraints`, `m_ImportLights`, `m_ImportMaterials`, `m_ImportPhysicalCameras`, `m_ImportVisibility`, `m_InternalIDToNameTable`, `m_MaterialImportMode`, `m_MaterialLocation`, `m_Materials`, `m_NodeNameCollisionStrategy`, `m_PreserveHierarchy`, `m_PreviousCalculatedGlobalScale`, `m_RemapMaterialsIfMaterialImportModeIsNone`, `m_RemoveConstantScaleCurves`, `m_ResampleCurves`, `m_ResampleRotations`, `m_RigImportErrors`, `m_RigImportWarnings`, `m_SortHierarchyByName`, `m_StrictVertexDataChecks`, `m_SupportsEmbeddedMaterials`, `m_UseSRGBMaterialColor`, `m_UsedFileIDs`, `maxBonesPerVertex`, `maximumMeshLod`, `meshLodGenerationFlags`, `meshOptimizationFlags`, `minBoneWeight`, `normalCalculationMode`, `normalSmoothingSource`, `optimizeBones`, `optimizeMeshForGPU`, `secondaryUVMarginMethod`, `secondaryUVMinLightmapResolution`, `secondaryUVMinObjectScale`, `skinWeightsMode`, `splitTangentsAcrossUV` |
| `MonoImporter` | AssetImporter | 1035 | `executionOrder`, `icon`, `m_DefaultReferences`, `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `MovieImporter` | AssetImporter | 1044 | `m_Name`, `m_Quality`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_LinearTexture`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `MultiArtifactTestImporter` | AssetImporter | 1223240404 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `NativeFormatImporter` | AssetImporter | 1034 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_MainObjectFileID`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `PVRImporter` | AssetImporter | 1052 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UserData` |
| `PackageManifestImporter` | AssetImporter | 1896753126 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `PluginImporter` | AssetImporter | 1050 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExecutionOrder`, `m_IconMap`, `m_IsPreloaded`, `m_Name`, `m_Output`, `m_PlatformData`, `m_UserData`, `m_DefineConstraints`, `m_ExternalObjects`, `m_IsExplicitlyReferenced`, `m_IsOverridable`, `m_UsedFileIDs`, `m_ValidateReferences` |
| `PrefabImporter` | AssetImporter | 468431735 | `m_AddedObjectFileIDs`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_IsPrefabVariant`, `m_Name`, `m_UserData`, `m_UnableToImportOnPreviousDomainReload`, `m_UsedFileIDs`, `m_VariantParentGUID` |
| `PreviewImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `RayTracingShaderImporter` | AssetImporter | 747330370 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData`, `m_CurrentAPIMask` |
| `ReferencesArtifactGenerator` | AssetImporter | 1114811875 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `RoslynAdditionalFileImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `RoslynAnalyzerConfigImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `RuleSetFileImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `ScriptedImporter` | AssetImporter | 2089858483 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_Name`, `m_Script`, `m_UserData`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_InternalIDToNameTable`, `m_UsedFileIDs` |
| `ShaderImporter` | AssetImporter | 1007 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_DefaultTextures`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_NonModifiableTextures`, `m_OldHashIdentity`, `m_PreprocessorOverride`, `m_UsedFileIDs`, `m_UserData` |
| `ShaderIncludeImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData` |
| `SpeedTreeImporter` | AssetImporter | 1110 | `m_AlphaTestRef`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_BestWindQuality`, `m_BillboardTransitionCrossFadeWidth`, `m_EnableSmoothLODTransition`, `m_FadeOutWidth`, `m_HasBillboard`, `m_HueVariation`, `m_LODSettings`, `m_MainColor`, `m_Name`, `m_ScaleFactor`, `m_UserData`, `m_AnimateCrossFading`, `m_EnableBumpMapping`, `m_EnableHueVariation`, `m_EnableLightProbes`, `m_EnableShadowCasting`, `m_EnableShadowReceiving`, `m_EnableSubsurfaceScattering`, `m_ExternalObjects`, `m_FileIDType`, `m_GenerateColliders`, `m_GenerateRigidbody`, `m_MaterialLocation`, `m_MaterialVersion`, `m_Materials`, `m_MotionVectorModeEnumValue`, `m_ReflectionProbeEnumValue`, `m_SelectedWindQuality`, `m_Shininess`, `m_SpecColor`, `m_SupportsEmbeddedMaterials`, `m_UnitConversionEnumValue`, `m_UsedFileIDs` |
| `SpriteAtlasImporter` | AssetImporter | 1210832254 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UsedFileIDs`, `m_UserData`, `m_BindAsDefault`, `m_PackingSettings`, `m_PlatformSettings`, `m_SecondaryTextureSettings`, `m_TextureSettings`, `m_VariantMultiplier` |
| `StyleSheetImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_Name`, `m_UserData` |
| `SubstanceImporter` | AssetImporter | 1112 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_DeletedPrototypes`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_IsFirstImport`, `m_MaterialImportOutputs`, `m_MaterialInstances`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `TextScriptImporter` | AssetImporter | 1031 | `m_Name`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UsedFileIDs`, `m_UserData` |
| `TextureImporter` | AssetImporter | 1006 | `m_BorderMipMap`, `m_ConvertToNormalMap`, `m_EnableMipMap`, `m_ExternalNormalMap`, `m_FadeOut`, `m_GenerateCubemap`, `m_GrayScaleToAlpha`, `m_HeightScale`, `m_IsReadable`, `m_Lightmap`, `m_MaxTextureSize`, `m_MipMapFadeDistanceEnd`, `m_MipMapFadeDistanceStart`, `m_MipMapMode`, `m_NPOTScale`, `m_Name`, `m_NormalMapFilter`, `m_TextureFormat`, `m_TextureSettings`, `m_TextureType`, `correctGamma`, `m_Alignment`, `m_AllowsAlphaSplitting`, `m_AlphaIsTransparency`, `m_AlphaTestReferenceValue`, `m_AlphaUsage`, `m_ApplyGammaDecoding`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_BuildTargetSettings`, `m_CompressionQuality`, `m_CompressionQualitySet`, `m_CookieLightType`, `m_CorrectGamma`, `m_CubemapConvolution`, `m_CubemapConvolutionExponent`, `m_CubemapConvolutionSteps`, `m_ExternalObjects`, `m_FileIDToRecycleName`, `m_FlipGreenChannel`, `m_FlipbookColumns`, `m_FlipbookRows`, `m_IgnoreMasterTextureLimit`, `m_IgnoreMipmapLimit`, `m_IgnorePngGamma`, `m_InternalIDToNameTable`, `m_LinearTexture`, `m_MaxTextureSizeSet`, `m_MipMapsPreserveCoverage`, `m_MipmapLimitGroupName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_Output`, `m_PSDRemoveMatte`, `m_PSDShowRemoveMatteOption`, `m_PlatformSettings`, `m_PushPullDilation`, `m_RGBM`, `m_RecommendedTextureFormat`, `m_SeamlessCubemap`, `m_SingleChannelComponent`, `m_SourceTextureInformation`, `m_SpriteBorder`, `m_SpriteExtrude`, `m_SpriteGenerateFallbackPhysicsShape`, `m_SpriteMeshType`, `m_SpriteMode`, `m_SpritePackingTag`, `m_SpritePivot`, `m_SpritePixelsToUnits`, `m_SpriteSheet`, `m_SpriteTessellationDetail`, `m_StreamingMipmaps`, `m_StreamingMipmapsPriority`, `m_Swizzle`, `m_TextureFormatSet`, `m_TextureShape`, `m_UsedFileIDs`, `m_UserData`, `m_VTOnly`, `m_sRGBTexture` |
| `TrueTypeFontImporter` | AssetImporter | 1042 | `m_FontNames`, `m_FontSize`, `m_ForceTextureCase`, `m_IncludeFontData`, `m_Name`, `m_AscentCalculationMode`, `m_AssetBundleName`, `m_AssetBundleVariant`, `m_CharacterPadding`, `m_CharacterSpacing`, `m_CustomCharacters`, `m_ExternalObjects`, `m_FallbackFontReferences`, `m_FileIDToRecycleName`, `m_FontColor`, `m_FontName`, `m_FontRenderingMode`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_Output`, `m_RenderMode`, `m_ShouldRoundAdvanceValue`, `m_Style`, `m_Use2xBehaviour`, `m_UseLegacyBoundsCalculation`, `m_UsedFileIDs`, `m_UserData` |
| `VideoClipImporter` | AssetImporter | 1127 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ColorSpace`, `m_Deinterlace`, `m_EncodeAlpha`, `m_EndFrame`, `m_FlipHorizontal`, `m_FlipVertical`, `m_FrameRange`, `m_Name`, `m_Output`, `m_StartFrame`, `m_TargetSettings`, `m_UserData`, `m_AudioImportMode`, `m_ExternalObjects`, `m_FrameCount`, `m_FrameRate`, `m_ImportAudio`, `m_IsColorLinear`, `m_OriginalHeight`, `m_OriginalWidth`, `m_PixelAspectRatioDenominator`, `m_PixelAspectRatioNumerator`, `m_Quality`, `m_SourceAudioChannelCount`, `m_SourceAudioSampleRate`, `m_SourceFileSize`, `m_SourceHasAlpha`, `m_UseLegacyImporter`, `m_UsedFileIDs` |
| `VisualEffectImporter` | AssetImporter | 2058629510 | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_ExternalObjects`, `m_Name`, `m_UserData`, `m_Template`, `m_UseAsTemplate`, `m_UsedFileIDs` |
| `AudioContainerElement` | NamedObject | — | `m_AudioClip`, `m_Enabled`, `m_Name`, `m_Volume` |
| `AudioMixer` | NamedObject | 240 | `m_EnableSuspend`, `m_MasterGroup`, `m_MixerConstant`, `m_Name`, `m_OutputGroup`, `m_Snapshots`, `m_StartSnapshot`, `m_SuspendThreshold`, `m_UpdateMode` |
| `AudioMixerController` | AudioMixer | 241 | `m_EnableSuspend`, `m_MasterGroup`, `m_MixerConstant`, `m_Name`, `m_OutputGroup`, `m_Snapshots`, `m_StartSnapshot`, `m_SuspendThreshold`, `m_UpdateMode` |
| `AudioMixerEffectController` | NamedObject | 244 | `m_Bypass`, `m_EffectID`, `m_EffectName`, `m_EnableWetMix`, `m_MixLevel`, `m_Name`, `m_Parameters`, `m_SendTarget` |
| `AudioMixerGroup` | NamedObject | 273 | `m_AudioMixer`, `m_Children`, `m_GroupID`, `m_Name` |
| `AudioMixerGroupController` | AudioMixerGroup | 243 | `m_AudioMixer`, `m_Children`, `m_GroupID`, `m_Name` |
| `AudioMixerSnapshot` | NamedObject | 272 | `m_AudioMixer`, `m_Name`, `m_SnapshotID` |
| `AudioMixerSnapshotController` | AudioMixerSnapshot | 245 | `m_AudioMixer`, `m_Name`, `m_SnapshotID` |
| `AudioResource` | NamedObject | — | `m_Name` |
| `AudioRandomContainer` | AudioResource | — | `m_AutomaticTriggerMode`, `m_AutomaticTriggerTime`, `m_AutomaticTriggerTimeRandomizationEnabled`, `m_AutomaticTriggerTimeRandomizationRange`, `m_AvoidRepeatingLast`, `m_Elements`, `m_LoopCount`, `m_LoopCountRandomizationEnabled`, `m_LoopCountRandomizationRange`, `m_LoopMode`, `m_Name`, `m_Pitch`, `m_PitchRandomizationEnabled`, `m_PitchRandomizationRange`, `m_PlaybackMode`, `m_TriggerMode`, `m_Volume`, `m_VolumeRandomizationEnabled`, `m_VolumeRandomizationRange` |
| `SampleClip` | AudioResource | 271 | `m_Name` |
| `AudioClip` | SampleClip | 83 | `m_Name`, `m_3D`, `m_Ambisonic`, `m_AudioData`, `m_BitsPerSample`, `m_Channels`, `m_CompressionFormat`, `m_Format`, `m_Frequency`, `m_IsTrackerFormat`, `m_Legacy3D`, `m_Length`, `m_LoadInBackground`, `m_LoadType`, `m_PreloadAudioData`, `m_Resource`, `m_Stream`, `m_SubsoundIndex`, `m_Type`, `m_UseHardware` |
| `Avatar` | NamedObject | 90 | `m_Avatar`, `m_AvatarSize`, `m_Name`, `m_TOS`, `m_HumanDescription` |
| `AvatarMask` | NamedObject | 319 | `m_Elements`, `m_Mask`, `m_Name` |
| `AvatarSkeletonMask` | NamedObject | — | `elements`, `m_Name` |
| `BaseAnimationTrack` | NamedObject, ABC | 110 | — |
| `NewAnimationTrack` | BaseAnimationTrack | 118 | `m_ClassID`, `m_Curves`, `m_Name` |
| `BillboardAsset` | NamedObject | 226 | `bottom`, `height`, `imageTexCoords`, `indices`, `m_Name`, `material`, `vertices`, `width`, `rotated` |
| `BlobObject` | NamedObject | — | `m_BlobData`, `m_BlobTypeHash`, `m_Name`, `m_NestedBlobObjectReferenceOffsets`, `m_NestedBlobObjectReferences` |
| `BlockShaderContainer` | NamedObject | — | `blob`, `dependencies`, `generatedPaths`, `guid`, `m_Name`, `state` |
| `BlockShaderErrors` | NamedObject | — | `errors`, `filePath`, `m_Name` |
| `BlockShaderSyntaxTree` | NamedObject | — | `filePath`, `m_Name`, `source` |
| `BuildReport` | NamedObject | 1125 | `m_Appendices`, `m_BuildSteps`, `m_Files`, `m_Name`, `m_Summary`, `m_RootAssetPaths` |
| `CachedSpriteAtlas` | NamedObject | 214 | `frames`, `textures`, `alphaTextures` |
| `CachedSpriteAtlasRuntimeData` | NamedObject | 644342135 | `alphaTextures`, `frames`, `textures`, `currentPackingHash` |
| `ComputeShader` | NamedObject | 72 | `m_Name`, `constantBuffers`, `kernels`, `variants` |
| `D3D12DeviceFilterLists` | NamedObject | — | `m_AllowFilterList`, `m_DenyFilterList`, `m_GraphicsJobsFilterList`, `m_Name` |
| `DefaultAsset` | NamedObject | 1029 | `m_Name`, `m_ErrorCode`, `m_IsWarning`, `m_Message` |
| `BrokenPrefabAsset` | DefaultAsset | — | `m_BrokenParentPrefab`, `m_IsPrefabFileValid`, `m_IsVariant`, `m_IsWarning`, `m_Message`, `m_Name`, `m_ErrorCode` |
| `SceneAsset` | DefaultAsset | 1032 | `m_Name`, `m_ErrorCode`, `m_IsWarning`, `m_Message` |
| `EditorProjectAccess` | NamedObject | 426301858 | `m_Name` |
| `Flare` | NamedObject | 121 | `m_Elements`, `m_FlareTexture`, `m_Name`, `m_TextureLayout`, `m_UseFog` |
| `Font` | NamedObject | 128 | `m_Ascent`, `m_AsciiStartOffset`, `m_CharacterRects`, `m_ConvertCase`, `m_DefaultMaterial`, `m_DefaultStyle`, `m_FontData`, `m_FontNames`, `m_FontSize`, `m_KerningValues`, `m_LineSpacing`, `m_Name`, `m_Texture`, `m_CharacterPadding`, `m_CharacterSpacing`, `m_Descent`, `m_FallbackFonts`, `m_FontCountX`, `m_FontCountY`, `m_FontRenderingMode`, `m_GridFont`, `m_Kerning`, `m_PerCharacterKerning`, `m_PixelScale`, `m_ShouldRoundAdvanceValue`, `m_Tracking`, `m_UseLegacyBoundsCalculation` |
| `GameObjectRecorder` | NamedObject | 1268269756 | `m_Name` |
| `GraphicsStateCollection` | NamedObject | — | `m_DeviceRenderer`, `m_Name`, `m_QualityLevelName`, `m_RenderPassInfoMap`, `m_RenderStateMap`, `m_RuntimePlatform`, `m_VariantInfoMap`, `m_Version`, `m_VertexLayoutInfoMap` |
| `HumanTemplate` | NamedObject | 1105 | `m_BoneTemplate`, `m_Name` |
| `ImportLog` | NamedObject | — | `m_Logs`, `m_Name` |
| `LightProbes` | NamedObject | 197, 258 | `m_Name`, `bakedCoefficients`, `bakedPositions`, `hullRays`, `m_BakedCoefficients`, `m_BakedLightOcclusion`, `m_Data`, `m_HasBeenEdited`, `tetrahedra` |
| `LightingDataAsset` | NamedObject | 1120 | `m_BakedAmbientProbeInLinear`, `m_BakedReflectionProbeCubemaps`, `m_BakedReflectionProbes`, `m_EnlightenData`, `m_EnlightenSceneMapping`, `m_EnlightenSceneMappingRendererIDs`, `m_LightProbes`, `m_LightmappedRendererData`, `m_LightmappedRendererDataIDs`, `m_Lightmaps`, `m_Lights`, `m_Name`, `m_AOTextures`, `m_BakedLightIndices`, `m_BakedReflectionProbeCubemapCacheFiles`, `m_EnlightenDataVersion`, `m_LightBakingOutputs`, `m_LightmapsCacheFiles`, `m_LightmapsMode`, `m_Scene`, `m_SceneGUID` |
| `LightingDataAssetParent` | NamedObject | 1325145578 | `m_Name` |
| `LightingSettings` | NamedObject | 850595691 | `m_AlbedoBoost`, `m_BounceScale`, `m_EnableBakedLightmaps`, `m_EnableRealtimeLightmaps`, `m_Name`, `m_RealtimeEnvironmentLighting`, `m_UsingShadowmask`, `m_AO`, `m_AOMaxDistance`, `m_BakeBackend`, `m_BakeResolution`, `m_CompAOExponent`, `m_CompAOExponentDirect`, `m_ExportTrainingData`, `m_ExtractAO`, `m_FilterMode`, `m_FinalGather`, `m_FinalGatherFiltering`, `m_FinalGatherRayCount`, `m_ForceUpdates`, `m_ForceWhiteAlbedo`, `m_GIWorkflowMode`, `m_IndirectOutputScale`, `m_LightmapMaxSize`, `m_LightmapParameters`, `m_LightmapsBakeMode`, `m_MixedBakeMode`, `m_PVRBounces`, `m_PVRCulling`, `m_PVRDenoiserTypeAO`, `m_PVRDenoiserTypeDirect`, `m_PVRDenoiserTypeIndirect`, `m_PVRDirectSampleCount`, `m_PVREnvironmentMIS`, `m_PVREnvironmentReferencePointCount`, `m_PVREnvironmentSampleCount`, `m_PVRFilterTypeAO`, `m_PVRFilterTypeDirect`, `m_PVRFilterTypeIndirect`, `m_PVRFilteringAtrousPositionSigmaAO`, `m_PVRFilteringAtrousPositionSigmaDirect`, `m_PVRFilteringAtrousPositionSigmaIndirect`, `m_PVRFilteringGaussRadiusAO`, `m_PVRFilteringGaussRadiusDirect`, `m_PVRFilteringGaussRadiusIndirect`, `m_PVRFilteringMode`, `m_PVRSampleCount`, `m_PVRSampling`, `m_Padding`, `m_RealtimeResolution`, `m_TextureCompression`, `m_TrainingDataDestination` |
| `LightmapParameters` | NamedObject | 1113 | `AOAntiAliasingSamples`, `AOQuality`, `antiAliasingSamples`, `backFaceTolerance`, `bakedLightmapTag`, `blurRadius`, `clusterResolution`, `directLightQuality`, `edgeStitching`, `irradianceBudget`, `irradianceQuality`, `isTransparent`, `m_Name`, `modellingTolerance`, `resolution`, `systemTag`, `limitLightmapCount`, `maxLightmapCount`, `pushoff` |
| `LocalizationAsset` | NamedObject | 2083778819 | `Editor_Asset`, `Locale_ISO_Code`, `String_Table`, `m_Name` |
| `Material` | NamedObject | 21 | `m_Name`, `m_SavedProperties`, `m_Shader`, `disabledShaderPasses`, `m_BuildTextureStacks`, `m_CustomRenderQueue`, `m_DoubleSidedGI`, `m_EnableInstancingVariants`, `m_InvalidKeywords`, `m_LightmapFlags`, `m_ShaderKeywords`, `m_ValidKeywords`, `stringTagMap` |
| `ProceduralMaterial` | Material | 185 | `m_Name`, `m_SavedProperties`, `m_Shader`, `disabledShaderPasses`, `m_AnimationUpdateRate`, `m_BuildTextureStacks`, `m_CacheSize`, `m_CustomRenderQueue`, `m_DoubleSidedGI`, `m_EnableInstancingVariants`, `m_Flags`, `m_GenerateMipmaps`, `m_Hash`, `m_Height`, `m_Inputs`, `m_InvalidKeywords`, `m_LightmapFlags`, `m_LoadingBehavior`, `m_MaximumSize`, `m_PrototypeName`, `m_ShaderKeywords`, `m_SubstancePackage`, `m_Textures`, `m_ValidKeywords`, `m_Width`, `stringTagMap` |
| `Mesh` | NamedObject | 43 | `m_BindPose`, `m_CompressedMesh`, `m_IndexBuffer`, `m_LocalAABB`, `m_MeshCompression`, `m_MeshUsageFlags`, `m_Name`, `m_SubMeshes`, `m_BakedConvexCollisionMesh`, `m_BakedTriangleCollisionMesh`, `m_BoneNameHashes`, `m_BonesAABB`, `m_CollisionTriangles`, `m_CollisionVertexCount`, `m_Colors`, `m_CookingOptions`, `m_IndexFormat`, `m_IsReadable`, `m_KeepIndices`, `m_KeepVertices`, `m_MeshLodInfo`, `m_MeshMetrics_0_`, `m_MeshMetrics_1_`, `m_Normals`, `m_RootBoneNameHash`, `m_ShapeVertices`, `m_Shapes`, `m_Skin`, `m_StreamCompression`, `m_StreamData`, `m_Tangents`, `m_UV`, `m_UV1`, `m_Use16BitIndices`, `m_VariableBoneCountWeights`, `m_VertexData`, `m_Vertices` |
| `Motion` | NamedObject, ABC | 207 | — |
| `AnimationClip` | Motion | 74 | `m_Bounds`, `m_Compressed`, `m_CompressedRotationCurves`, `m_Events`, `m_FloatCurves`, `m_Name`, `m_PositionCurves`, `m_RotationCurves`, `m_SampleRate`, `m_ScaleCurves`, `m_WrapMode`, `m_AnimationType`, `m_ClipBindingConstant`, `m_EulerCurves`, `m_HasGenericRootTransform`, `m_HasMotionFloatCurves`, `m_Legacy`, `m_MuscleClip`, `m_MuscleClipSize`, `m_PPtrCurves`, `m_UseHighQualityCurve` |
| `PreviewAnimationClip` | AnimationClip | 1108 | `m_Bounds`, `m_ClipBindingConstant`, `m_Compressed`, `m_CompressedRotationCurves`, `m_EulerCurves`, `m_Events`, `m_FloatCurves`, `m_Legacy`, `m_MuscleClip`, `m_MuscleClipSize`, `m_Name`, `m_PPtrCurves`, `m_PositionCurves`, `m_RotationCurves`, `m_SampleRate`, `m_ScaleCurves`, `m_UseHighQualityCurve`, `m_WrapMode`, `m_HasGenericRootTransform`, `m_HasMotionFloatCurves` |
| `BlendTree` | Motion | 206 | `m_Childs`, `m_MaxThreshold`, `m_MinThreshold`, `m_Name`, `m_UseAutomaticThresholds`, `m_BlendEvent`, `m_BlendEventY`, `m_BlendParameter`, `m_BlendParameterY`, `m_BlendType`, `m_NormalizedBlendValues` |
| `NavMeshData` | NamedObject | 238 | `m_HeightMeshes`, `m_Heightmaps`, `m_Name`, `m_NavMeshTiles`, `m_OffMeshLinks`, `m_AgentTypeID`, `m_NavMeshBuildSettings`, `m_NavMeshParams`, `m_Position`, `m_Rotation`, `m_SourceBounds` |
| `NavMeshObsolete` | NamedObject | 194 | `m_Name` |
| `OcclusionCullingData` | NamedObject | 363 | `m_Name`, `m_PVSData`, `m_Scenes` |
| `PhysicsMaterial` | NamedObject | — | `m_Name`, `bounceCombine`, `bounciness`, `dynamicFriction`, `frictionCombine`, `m_BounceCombine`, `m_Bounciness`, `m_DynamicFriction`, `m_FrictionCombine`, `m_StaticFriction`, `staticFriction` |
| `PhysicsMaterial2D` | NamedObject | 62 | `bounciness`, `friction`, `m_Name`, `m_BounceCombine`, `m_FrictionCombine` |
| `PreloadData` | NamedObject | 150 | `m_Assets`, `m_Name`, `m_Dependencies`, `m_ExplicitDataLayout` |
| `Preset` | NamedObject | 181963792 | `m_Name`, `m_Properties`, `m_TargetType`, `m_CoupledProperties`, `m_CoupledType`, `m_ExcludedProperties` |
| `RayTracingShader` | NamedObject | 825902497 | `m_MaxRecursionDepth`, `m_Name`, `variants`, `m_EnableRayPayloadSizeChecks` |
| `RoslynAdditionalFileAsset` | NamedObject | — | `m_Name` |
| `RoslynAnalyzerConfigAsset` | NamedObject | — | `m_Name` |
| `RuntimeAnimatorController` | NamedObject | 93 | `m_AnimationClips`, `m_Controller`, `m_ControllerSize`, `m_Name`, `m_TOS` |
| `AnimatorController` | RuntimeAnimatorController | 91 | `m_AnimationClips`, `m_Controller`, `m_ControllerSize`, `m_Name`, `m_TOS`, `m_EvaluateTransitionsOnStart`, `m_MultiThreadedStateMachine`, `m_StateMachineBehaviourVectorDescription`, `m_StateMachineBehaviours` |
| `AnimatorOverrideController` | RuntimeAnimatorController | 221 | `m_Clips`, `m_Controller`, `m_Name` |
| `Shader` | NamedObject | 48 | `m_Name`, `compressedBlob`, `compressedLengths`, `decompressedLengths`, `decompressedSize`, `m_AssetGUID`, `m_AssetLocalIdentifierInFile`, `m_Dependencies`, `m_NonModifiableTextures`, `m_ParsedForm`, `m_PathName`, `m_Script`, `m_ShaderIsBaked`, `m_SubProgramBlob`, `offsets`, `platforms`, `stageCounts` |
| `ShaderVariantCollection` | NamedObject | 200 | `m_Name`, `m_Shaders` |
| `SpeedTreeWindAsset` | NamedObject | 228 | `m_Name`, `m_Config8`, `m_Config9`, `m_Wind`, `m_eVersion` |
| `Sprite` | NamedObject | 213 | `m_Extrude`, `m_Name`, `m_Offset`, `m_PixelsToUnits`, `m_RD`, `m_Rect`, `m_AtlasTags`, `m_Bones`, `m_Border`, `m_IsPolygon`, `m_PhysicsShape`, `m_Pivot`, `m_RenderDataKey`, `m_ScriptableObjects`, `m_SpriteAtlas` |
| `SpriteAtlas` | NamedObject | 687078895 | `m_IsVariant`, `m_Name`, `m_PackedSpriteNamesToIndex`, `m_PackedSprites`, `m_RenderDataMap`, `m_Tag`, `m_Guid` |
| `SpriteAtlasAsset` | NamedObject | 612988286 | `m_ImporterData`, `m_IsVariant`, `m_MasterAtlas`, `m_Name`, `m_ScriptablePacker` |
| `SubstanceArchive` | NamedObject | 184 | `m_Name`, `m_PackageData` |
| `TerrainData` | NamedObject | 156 | `m_DetailDatabase`, `m_Heightmap`, `m_Name`, `m_SplatDatabase`, `m_PreloadShaders` |
| `TerrainLayer` | NamedObject | 1953259897 | `m_DiffuseRemapMax`, `m_DiffuseRemapMin`, `m_DiffuseTexture`, `m_MaskMapRemapMax`, `m_MaskMapRemapMin`, `m_MaskMapTexture`, `m_Metallic`, `m_Name`, `m_NormalMapTexture`, `m_NormalScale`, `m_Smoothness`, `m_Specular`, `m_TileOffset`, `m_TileSize`, `m_SmoothnessSource` |
| `TextAsset` | NamedObject | 49 | `m_Name`, `m_Script`, `m_PathName` |
| `AssemblyDefinitionAsset` | TextAsset | 1152215463 | `m_Name`, `m_Script` |
| `AssemblyDefinitionReferenceAsset` | TextAsset | 662584278 | `m_Name`, `m_Script` |
| `MonoScript` | TextAsset | 115 | `m_AssemblyName`, `m_ClassName`, `m_ExecutionOrder`, `m_Name`, `m_Namespace`, `m_PropertiesHash`, `m_IsEditorScript` |
| `PackageManifest` | TextAsset | 1896753125 | `m_Name`, `m_Script` |
| `RuleSetFileAsset` | TextAsset | — | `m_Name`, `m_Script` |
| `ShaderInclude` | TextAsset | — | `m_Name`, `m_Script` |
| `Texture` | NamedObject, ABC | 27 | — |
| `BaseVideoTexture` | Texture, ABC | 237 | — |
| `WebCamTexture` | BaseVideoTexture | 158 | `m_Name`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IsAlphaChannelOptional` |
| `CubemapArray` | Texture | 188 | `image_data`, `m_ColorSpace`, `m_CubemapCount`, `m_DataSize`, `m_Format`, `m_IsReadable`, `m_MipCount`, `m_Name`, `m_TextureSettings`, `m_Width`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IsAlphaChannelOptional`, `m_StreamData`, `m_UsageMode` |
| `LowerResBlitTexture` | Texture | 1480428607 | `m_Name`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IsAlphaChannelOptional` |
| `MovieTexture` | Texture | 152 | `m_Name`, `m_AudioClip`, `m_ColorSpace`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IsAlphaChannelOptional`, `m_Loop`, `m_MovieData` |
| `ProceduralTexture` | Texture | 186 | `m_Name`, `AlphaSource`, `AlphaSourceIsGrayscale`, `Format`, `Type`, `m_AlphaSourceIsInverted`, `m_AlphaSourceUID`, `m_BakedData`, `m_BakedParameters`, `m_ColorSpace`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IsAlphaChannelOptional`, `m_LightmapFormat`, `m_Mipmaps`, `m_SubstanceMaterial`, `m_SubstanceTextureUID`, `m_TextureParameters`, `m_TextureSettings` |
| `RenderTexture` | Texture | 84 | `m_ColorFormat`, `m_Height`, `m_MipMap`, `m_Name`, `m_TextureSettings`, `m_Width`, `m_AntiAliasing`, `m_BindMS`, `m_DepthFormat`, `m_DepthStencilFormat`, `m_Dimension`, `m_DownscaleFallback`, `m_EnableCompatibleFormat`, `m_EnableRandomWrite`, `m_ForcedFallbackFormat`, `m_GenerateMips`, `m_IsAlphaChannelOptional`, `m_IsCubemap`, `m_IsPowerOfTwo`, `m_MipCount`, `m_SRGB`, `m_ShadowSamplingMode`, `m_UseDynamicScale`, `m_UseDynamicScaleExplicit`, `m_VolumeDepth` |
| `CustomRenderTexture` | RenderTexture | 86 | `m_AntiAliasing`, `m_ColorFormat`, `m_CubemapFaceMask`, `m_CurrentUpdateZoneSpace`, `m_Dimension`, `m_DoubleBuffered`, `m_GenerateMips`, `m_Height`, `m_InitColor`, `m_InitMaterial`, `m_InitTexture`, `m_InitializationMode`, `m_Material`, `m_MipMap`, `m_Name`, `m_SRGB`, `m_ShaderPass`, `m_TextureSettings`, `m_UpdateMode`, `m_UpdatePeriod`, `m_UpdateZoneSpace`, `m_UpdateZones`, `m_VolumeDepth`, `m_Width`, `m_WrapUpdateZones`, `m_BindMS`, `m_DepthFormat`, `m_DepthStencilFormat`, `m_DownscaleFallback`, `m_EnableCompatibleFormat`, `m_EnableRandomWrite`, `m_ForcedFallbackFormat`, `m_InitSource`, `m_IsAlphaChannelOptional`, `m_MipCount`, `m_ShadowSamplingMode`, `m_UseDynamicScale`, `m_UseDynamicScaleExplicit` |
| `SparseTexture` | Texture | 171 | `m_ColorSpace`, `m_Format`, `m_Height`, `m_MipCount`, `m_Name`, `m_TextureSettings`, `m_Width`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IsAlphaChannelOptional` |
| `Texture2D` | Texture | 28 | `image_data`, `m_CompleteImageSize`, `m_Height`, `m_ImageCount`, `m_IsReadable`, `m_LightmapFormat`, `m_Name`, `m_TextureDimension`, `m_TextureFormat`, `m_TextureSettings`, `m_Width`, `m_ColorSpace`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IgnoreMasterTextureLimit`, `m_IgnoreMipmapLimit`, `m_IsAlphaChannelOptional`, `m_IsPreProcessed`, `m_MipCount`, `m_MipMap`, `m_MipmapLimitGroupName`, `m_MipsStripped`, `m_PlatformBlob`, `m_ReadAllowed`, `m_StreamData`, `m_StreamingMipmaps`, `m_StreamingMipmapsPriority` |
| `Cubemap` | Texture2D | 89 | `image_data`, `m_CompleteImageSize`, `m_Height`, `m_ImageCount`, `m_IsReadable`, `m_LightmapFormat`, `m_Name`, `m_TextureDimension`, `m_TextureFormat`, `m_TextureSettings`, `m_Width`, `m_ColorSpace`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IgnoreMasterTextureLimit`, `m_IgnoreMipmapLimit`, `m_IsAlphaChannelOptional`, `m_IsPreProcessed`, `m_MipCount`, `m_MipMap`, `m_MipmapLimitGroupName`, `m_MipsStripped`, `m_PlatformBlob`, `m_ReadAllowed`, `m_SourceTextures`, `m_StreamData`, `m_StreamingMipmaps`, `m_StreamingMipmapsPriority` |
| `Texture2DArray` | Texture | 187 | `image_data`, `m_ColorSpace`, `m_DataSize`, `m_Depth`, `m_Format`, `m_Height`, `m_IsReadable`, `m_MipCount`, `m_Name`, `m_TextureSettings`, `m_Width`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_IgnoreMipmapLimit`, `m_IsAlphaChannelOptional`, `m_MipmapLimitGroupName`, `m_MipsStripped`, `m_StreamData`, `m_UsageMode` |
| `Texture3D` | Texture | 117 | `image_data`, `m_Height`, `m_Name`, `m_TextureSettings`, `m_Width`, `m_ColorSpace`, `m_CompleteImageSize`, `m_DataSize`, `m_Depth`, `m_DownscaleFallback`, `m_ForcedFallbackFormat`, `m_Format`, `m_ImageCount`, `m_IsAlphaChannelOptional`, `m_IsReadable`, `m_LightmapFormat`, `m_MipCount`, `m_MipMap`, `m_ReadAllowed`, `m_StreamData`, `m_TextureDimension`, `m_TextureFormat`, `m_UsageMode` |
| `VideoClip` | NamedObject | 329 | `Height`, `Width`, `m_AudioChannelCount`, `m_AudioLanguage`, `m_AudioSampleRate`, `m_ExternalResources`, `m_Format`, `m_FrameCount`, `m_FrameRate`, `m_HasSplitAlpha`, `m_Name`, `m_OriginalPath`, `m_ProxyHeight`, `m_ProxyWidth`, `m_PixelAspecRatioDen`, `m_PixelAspecRatioNum`, `m_VideoShaders`, `m_sRGB` |
| `VisualEffectObject` | NamedObject, ABC | 2059678085 | — |
| `VisualEffectAsset` | VisualEffectObject | 2058629509 | `m_Infos`, `m_Name`, `m_Systems` |
| `VisualEffectSubgraph` | VisualEffectObject, ABC | 994735392 | — |
| `VisualEffectSubgraphBlock` | VisualEffectSubgraph | 994735404 | `m_Name` |
| `VisualEffectSubgraphOperator` | VisualEffectSubgraph | 994735403 | `m_Name` |
| `VisualEffectResource` | NamedObject | 2058629511 | `m_Graph`, `m_Infos`, `m_Name`, `m_ShaderSources`, `m_Systems` |
| `VulkanDeviceFilterLists` | NamedObject | — | `m_GfxJobFilterList`, `m_Name`, `m_VulkanAllowFilterList`, `m_VulkanDenyFilterList` |
| `EditorExtensionImpl` | Object | 1002 | `gFlattenedTypeTree`, `m_DataTemplate`, `m_Object`, `m_OverrideVariable`, `m_TemplateFather` |
| `EditorSettings` | Object | 159 | `m_AssetNamingUsesSpace`, `m_AssetPipelineMode`, `m_AsyncShaderCompilation`, `m_Bc7TextureCompressor`, `m_BlockShaders`, `m_CacheServerDownloadBatchSize`, `m_CacheServerEnableAuth`, `m_CacheServerEnableDownload`, `m_CacheServerEnableTls`, `m_CacheServerEnableUpload`, `m_CacheServerEndpoint`, `m_CacheServerMode`, `m_CacheServerNamespacePrefix`, `m_CacheServerValidationMode`, `m_CachingShaderPreprocessor`, `m_CollabEditorSettings`, `m_DefaultBehaviorMode`, `m_DisableCookiesInLightmapper`, `m_EnableEditorAsyncCPUTextureLoading`, `m_EnableEnlightenBakedGI`, `m_EnableRoslynAnalyzers`, `m_EnableTextureStreamingInEditMode`, `m_EnableTextureStreamingInPlayMode`, `m_EnterPlayModeOptions`, `m_EnterPlayModeOptionsEnabled`, `m_EtcTextureBestCompressor`, `m_EtcTextureCompressorBehavior`, `m_EtcTextureFastCompressor`, `m_EtcTextureNormalCompressor`, `m_ExternalVersionControlSupport`, `m_ForceAssetUnloadAndGCOnSceneLoad`, `m_GameObjectNamingDigits`, `m_GameObjectNamingScheme`, `m_HideBuildProfileClassicPlatforms`, `m_InspectorUseIMGUIDefaultInspector`, `m_LineEndingsForNewScripts`, `m_PrefabModeAllowAutoSave`, `m_PrefabRegularEnvironment`, `m_PrefabUIEnvironment`, `m_ProjectGenerationIncludedExtensions`, `m_ProjectGenerationRootNamespace`, `m_RecalculateEnvironmentLighting`, `m_ReferencedClipsExactNaming`, `m_RefreshImportMode`, `m_SerializationMode`, `m_SerializeInlineMappingsOnOneLine`, `m_ShadowmaskStitching`, `m_ShowLightmapResolutionOverlay`, `m_SpritePackerCacheSize`, `m_SpritePackerMode`, `m_SpritePackerPaddingPower`, `m_UnlockBlockShaders`, `m_UseLegacyProbeSampleCount`, `m_UserGeneratedProjectSuffix`, `m_WebSecurityEmulationEnabled`, `m_WebSecurityEmulationHostUrl` |
| `EditorUserBuildSettings` | Object | 1051 | `m_ActiveBuildTarget`, `m_AllowDebugging`, `m_ArchitectureFlags`, `m_BuildLocation`, `m_ConnectProfiler`, `m_Development`, `m_InstallInBuildFolder`, `m_SelectedAndroidSubtarget`, `m_SelectedBuildTargetGroup`, `m_SelectedStandaloneTarget`, `m_ActiveBuildPlatformGroupName`, `m_ActiveBuildProfile`, `m_ActiveBuildTargetGroup`, `m_ActivePlatformGuid`, `m_ActiveProfilePath`, `m_ActiveStandaloneBuildSubtarget`, `m_AndroidBuildSystem`, `m_AndroidBuildType`, `m_AndroidCreateSymbols`, `m_AndroidCreateSymbolsZip`, `m_AndroidCurrentDeploymentTargetId`, `m_AndroidDebugMinification`, `m_AndroidDeviceSocketAddress`, `m_AndroidReleaseMinification`, `m_AndroidUseLegacySdkTools`, `m_BuildAppBundle`, `m_BuildOutputToBuildMetadataMap`, `m_BuildScriptsOnly`, `m_BuildWithCodeCoverage`, `m_BuildWithDeepProfilingSupport`, `m_CompressFilesInPackage`, `m_CompressWithPsArc`, `m_CreateRomFileForSwitch`, `m_CreateSolutionFileForSwitch`, `m_DatalessPlayer`, `m_EnableDebugPadForSwitch`, `m_EnableHeadlessMode`, `m_EnableHeapInspectorForSwitch`, `m_EnableHostIOForSwitch`, `m_EnableMemoryTrackerForSwitch`, `m_EnableRomCompressionForSwitch`, `m_EnableUnpublishableErrorsForSwitch`, `m_ExplicitArrayBoundsChecks`, `m_ExplicitDivideByZeroChecks`, `m_ExplicitNullChecks`, `m_ExportAsGoogleAndroidProject`, `m_FacebookAccessToken`, `m_FacebookCreatePackageForSubmission`, `m_ForceInstallation`, `m_ForceOptimizeScriptCompilation`, `m_GenerateMetroReferenceProjects`, `m_GenerateNintendoSwitchShaderInfo`, `m_GenerateWSAReferenceProjects`, `m_HTCSScriptDebuggingForSwitch`, `m_Il2CppCodeGeneration`, `m_MovePackageToDiscOuterEdge`, `m_NVNAftermath`, `m_NVNAftermathLevel`, `m_NVNDrawValidation`, `m_NVNDrawValidationHeavy`, `m_NVNDrawValidationLight`, `m_NVNGraphicsDebuggerForSwitch`, `m_NVNShaderDebugging`, `m_NeedSubmissionMaterials`, `m_OverrideMaxTextureSize`, `m_OverrideTextureCompression`, `m_PS4HardwareTarget`, `m_PS5KeepPackageFiles`, `m_PS5WorkspaceName`, `m_PathOnRemoteDevice`, `m_PlatformSettings`, `m_RedirectWritesToHostMountForSwitch`, `m_RemoteDeviceAddress`, `m_RemoteDeviceExports`, `m_RemoteDeviceInfo`, `m_RemoteDeviceUsername`, `m_RomCompressionConfigForSwitch`, `m_RomCompressionLevelForSwitch`, `m_RomCompressionTypeForSwitch`, `m_SaveADFForSwitch`, `m_SelectedAndroidETC2Fallback`, `m_SelectedBlackBerryBuildType`, `m_SelectedBlackBerrySubtarget`, `m_SelectedBuildPlatformGroupName`, `m_SelectedBuildTarget`, `m_SelectedCompressionType`, `m_SelectedDiagnosticSetting`, `m_SelectedEmbeddedLinuxArchitecture`, `m_SelectedFacebookTarget`, `m_SelectedIOSBuildType`, `m_SelectedMetroBuildAndRunDeployTarget`, `m_SelectedMetroBuildType`, `m_SelectedMetroSDK`, `m_SelectedMetroTarget`, `m_SelectedPS3Subtarget`, `m_SelectedPS4Subtarget`, `m_SelectedPS5CompressionLevel`, `m_SelectedPS5CompressionType`, `m_SelectedPS5Subtarget`, `m_SelectedPSMSubtarget`, `m_SelectedPSP2Subtarget`, `m_SelectedQNXArchitecture`, `m_SelectedQNXOsVersion`, `m_SelectedStandaloneBuildSubtarget`, `m_SelectedTizenSubtarget`, `m_SelectedWSAArchitecture`, `m_SelectedWSABuildAndRunDeployTarget`, `m_SelectedWSAMinUWPSDK`, `m_SelectedWSASDK`, `m_SelectedWSASubtarget`, `m_SelectedWSAUWPBuildType`, `m_SelectedWSAUWPSDK`, `m_SelectedWSAUWPVSVersion`, `m_SelectedWebGLSubtarget`, `m_SelectedWiiDebugLevel`, `m_SelectedWiiSubtarget`, `m_SelectedWiiUBootMode`, `m_SelectedWiiUBuildOutput`, `m_SelectedWiiUDebugLevel`, `m_SelectedWindowsBuildAndRunDeployTarget`, `m_SelectedXboxOneDeployDrive`, `m_SelectedXboxOneDeployMethod`, `m_SelectedXboxRunMethod`, `m_SelectedXboxSubtarget`, `m_SymlinkLibraries`, `m_SymlinkSources`, `m_SymlinkTrampoline`, `m_UseLegacyNvnPoolAllocatorForSwitch`, `m_WSADotNetNativeEnabled`, `m_WaitForPlayerConnection`, `m_WaitForSwitchMemoryTrackerOnStartup`, `m_WebGLClientBrowserPath`, `m_WebGLClientBrowserType`, `m_WebGLClientPlatform`, `m_WebGLOptimizationLevel`, `m_WebGLUsePreBuiltUnityEngine`, `m_WebPlayerDeployOnline`, `m_WebPlayerNaClSupport`, `m_WebPlayerOfflineDeployment`, `m_WebPlayerStreamed`, `m_WiiUEnableNetAPI`, `m_WindowsDevicePortalAddress`, `m_WindowsDevicePortalUsername`, `m_WsaHolographicRemoting`, `m_XboxCompressedXex`, `m_XboxOneNetworkSharePath`, `m_XboxOneStreamingInstallLaunchChunkRange`, `m_XboxOneUsername`, `m_macosXcodeBuildConfig` |
| `EditorUserSettings` | Object | 162 | `m_VCAutomaticAdd`, `m_VCDebugCmd`, `m_VCDebugCom`, `m_VCDebugOut`, `m_ArtifactGarbageCollection`, `m_AssetPipelineMode`, `m_AssetPipelineMode2`, `m_CacheServerMode`, `m_CacheServers`, `m_CompressAssetsOnImport`, `m_ConfigSettings`, `m_ConfigValues`, `m_DesiredImportWorkerCount`, `m_IdleImportWorkerShutdownDelay`, `m_SemanticMergeMode`, `m_StandbyImportWorkerCount`, `m_VCAllowAsyncUpdate`, `m_VCHierarchyOverlayIcons`, `m_VCOtherOverlayIcons`, `m_VCOverlayIcons`, `m_VCOverwriteFailedCheckoutAssets`, `m_VCPassword`, `m_VCProjectOverlayIcons`, `m_VCScanLocalPackagesOnConnect`, `m_VCServer`, `m_VCShowFailedCheckout`, `m_VCUserName`, `m_VCWorkspace` |
| `EmptyObject` | Object | 277625683 | — |
| `GUIDSerializer` | Object | 1027 | `guidToPath` |
| `GameManager` | Object, ABC | 9 | — |
| `GlobalGameManager` | GameManager, ABC | 6 | — |
| `AnimationManager` | GlobalGameManager | — | — |
| `AudioManager` | GlobalGameManager | 11 | `Default_Speaker_Mode`, `Doppler_Factor`, `Rolloff_Scale`, `m_DSPBufferSize`, `m_Volume`, `m_AmbisonicDecoderPlugin`, `m_AudioFoundation`, `m_DisableAudio`, `m_OutputChannelLayout`, `m_OutputSamplingRate`, `m_RealVoiceCount`, `m_RequestedDSPBufferSize`, `m_SampleRate`, `m_SpatializerPlugin`, `m_SpeedOfSound`, `m_VirtualVoiceCount`, `m_VirtualizeEffects` |
| `BuildSettings` | GlobalGameManager | 141 | `enableDynamicBatching`, `hasAdvancedVersion`, `hasPROVersion`, `hasPublishingRights`, `hasShadows`, `isEducationalBuild`, `m_Version`, `buildGUID`, `buildTags`, `enableMultipleDisplays`, `enabledVRDevices`, `hasClusterRendering`, `hasLocalLightShadows`, `hasOculusPlugin`, `hasRenderTexture`, `hasSoftShadows`, `isDebugBuild`, `isEmbedded`, `isNoWatermarkBuild`, `isPrototypingBuild`, `isTrial`, `isWsaHolographicRemotingEnabled`, `levels`, `m_AuthToken`, `m_GraphicsAPIs`, `preloadedPlugins`, `runtimeClassHashes`, `scenes`, `scriptHashes`, `usesOnMouseEvents` |
| `CloudWebServicesManager` | GlobalGameManager | 301 | — |
| `ClusterInputManager` | GlobalGameManager | 236 | `m_Inputs` |
| `CrashReportManager` | GlobalGameManager | 304 | — |
| `DelayedCallManager` | GlobalGameManager | 98 | — |
| `GraphicsSettings` | GlobalGameManager | 30 | `m_AlwaysIncludedShaders`, `m_AllowEnlightenSupportForUpgradedProject`, `m_CameraRelativeLightCulling`, `m_CameraRelativeShadowCulling`, `m_CurrentRenderPipelineGlobalSettings`, `m_CustomRenderPipeline`, `m_DefaultRenderingLayerMask`, `m_Deferred`, `m_DeferredReflections`, `m_DepthNormals`, `m_LegacyDeferred`, `m_LensFlare`, `m_LightHalo`, `m_LightProbeOutsideHullStrategy`, `m_LightsUseCCT`, `m_LightsUseColorTemperature`, `m_LightsUseLinearIntensity`, `m_LogWhenShaderIsCompiled`, `m_MotionVectors`, `m_PreloadShadersBatchTimeLimit`, `m_PreloadedShaders`, `m_SRPDefaultSettings`, `m_ScreenSpaceShadows`, `m_ShaderDefinesPerShaderCompiler`, `m_ShaderSettings`, `m_ShaderSettings_Tier1`, `m_ShaderSettings_Tier2`, `m_ShaderSettings_Tier3`, `m_SpritesDefaultMaterial`, `m_TierSettings_Tier1`, `m_TierSettings_Tier2`, `m_TierSettings_Tier3`, `m_TransparencySortAxis`, `m_TransparencySortMode`, `m_VideoShadersIncludeMode` |
| `InputManager` | GlobalGameManager | 13 | `m_Axes`, `m_UsePhysicalKeys` |
| `MasterServerInterface` | GlobalGameManager | 155 | — |
| `MonoManager` | GlobalGameManager | 116 | `m_Scripts`, `m_AssemblyNames`, `m_AssemblyTypes`, `m_RuntimeClassHashes`, `m_ScriptHashes` |
| `MultiplayerManager` | GlobalGameManager | — | `m_ActiveMultiplayerRole`, `m_ActiveMultiplayerRoles` |
| `NavMeshProjectSettings` | GlobalGameManager | 126 | `areas`, `m_LastAgentTypeID`, `m_SettingNames`, `m_Settings` |
| `NetworkManager` | GlobalGameManager | 149 | `m_AssetToPrefab`, `m_DebugLevel`, `m_Sendrate` |
| `NotificationManager` | GlobalGameManager | — | — |
| `PerformanceReportingManager` | GlobalGameManager | 305 | — |
| `Physics2DSettings` | GlobalGameManager | 19, 51 | `m_DefaultMaterial`, `m_Gravity`, `m_LayerCollisionMatrix`, `m_PositionIterations`, `m_VelocityIterations`, `m_AngularSleepTolerance`, `m_AutoSimulation`, `m_AutoSyncTransforms`, `m_BaumgarteScale`, `m_BaumgarteTimeOfImpactScale`, `m_BounceThreshold`, `m_CallbacksOnDisable`, `m_ChangeStopsCallbacks`, `m_ContactThreshold`, `m_DefaultContactOffset`, `m_DeleteStopsCallbacks`, `m_JobOptions`, `m_LinearSleepTolerance`, `m_MaxAngularCorrection`, `m_MaxLinearCorrection`, `m_MaxRotationSpeed`, `m_MaxSubStepCount`, `m_MaxTranslationSpeed`, `m_MinPenetrationForPenalty`, `m_MinSubStepFPS`, `m_PhysicsLowLevelSettings`, `m_QueriesHitTriggers`, `m_QueriesStartInColliders`, `m_RaycastsHitTriggers`, `m_RaycastsStartInColliders`, `m_ReuseCollisionCallbacks`, `m_SimulationLayers`, `m_SimulationMode`, `m_TimeToSleep`, `m_UseSubStepContacts`, `m_UseSubStepping`, `m_VelocityThreshold` |
| `PhysicsManager` | GlobalGameManager | 55 | `m_BounceThreshold`, `m_DefaultMaterial`, `m_Gravity`, `m_LayerCollisionMatrix`, `m_AutoSimulation`, `m_AutoSyncTransforms`, `m_BroadphaseType`, `m_ClothGravity`, `m_ClothInterCollisionDistance`, `m_ClothInterCollisionSettingsToggle`, `m_ClothInterCollisionStiffness`, `m_ContactPairsMode`, `m_ContactsGeneration`, `m_CurrentBackendId`, `m_DefaultContactOffset`, `m_DefaultMaxAngluarSpeed`, `m_DefaultMaxAngularSpeed`, `m_DefaultMaxDepenetrationVelocity`, `m_DefaultSolverIterations`, `m_DefaultSolverVelocityIterations`, `m_EnableAdaptiveForce`, `m_EnableEnhancedDeterminism`, `m_EnablePCM`, `m_EnableUnifiedHeightmaps`, `m_FastMotionThreshold`, `m_FrictionType`, `m_GenerateOnTriggerStayEvents`, `m_ImprovedPatchFriction`, `m_IncrementalStaticBroadphase`, `m_InvokeCollisionCallbacks`, `m_LogVerbosity`, `m_MaxAngularVelocity`, `m_MinPenetrationForPenalty`, `m_QueriesHitBackfaces`, `m_QueriesHitTriggers`, `m_RaycastsHitTriggers`, `m_ReleaseSceneBuffers`, `m_ReuseCollisionCallbacks`, `m_SceneBuffersReleaseInterval`, `m_ScratchBufferChunkCount`, `m_SimulationMode`, `m_SleepAngularVelocity`, `m_SleepThreshold`, `m_SleepVelocity`, `m_SolverIterationCount`, `m_SolverType`, `m_SolverVelocityIterations`, `m_WorldBounds`, `m_WorldSubdivisions` |
| `PlayerSettings` | GlobalGameManager | 129 | `AndroidProfiler`, `allowedAutorotateToLandscapeLeft`, `allowedAutorotateToLandscapeRight`, `allowedAutorotateToPortrait`, `allowedAutorotateToPortraitUpsideDown`, `companyName`, `defaultScreenHeight`, `defaultScreenHeightWeb`, `defaultScreenOrientation`, `defaultScreenWidth`, `defaultScreenWidthWeb`, `productName`, `runInBackground`, `targetDevice`, `use32BitDisplayBuffer`, `useMacAppStoreValidation`, `useOSAutorotation`, `usePlayerLog`, `AID`, `AndroidEnableSustainedPerformanceMode`, `AndroidFilterTouchesWhenObscured`, `AndroidLicensePublicKey`, `D3DHDRBitDepth`, `Force_IOS_Speakers_When_Recording`, `Override_IPod_Music`, `Prepare_IOS_For_Recording`, `accelerometerFrequency`, `activeInputHandler`, `adjustIOSFPSUsingThermalState`, `allowFullscreenSwitch`, `allowHDRDisplaySupport`, `allowedHttpConnections`, `androidApplicationEntry`, `androidAutoRotationBehavior`, `androidBlitType`, `androidDefaultWindowHeight`, `androidDefaultWindowWidth`, `androidDisplayOptions`, `androidFullscreenMode`, `androidMaxAspectRatio`, `androidMinAspectRatio`, `androidMinimumWindowHeight`, `androidMinimumWindowWidth`, `androidPredictiveBackSupport`, `androidRenderOutsideSafeArea`, `androidRequestedVisibleInsets`, `androidResizableWindow`, `androidResizeableActivity`, `androidShowActivityIndicatorOnLoading`, `androidStartInFullscreen`, `androidSupportedAspectRatio`, `androidSystemBarsBehavior`, `androidUseSwappy`, `androidVulkanAllowFilterList`, `androidVulkanDenyFilterList`, `androidVulkanDeviceFilterListAsset`, `audioSpatialExperience`, `bakeCollisionMeshes`, `bundleIdentifier`, `bundleVersion`, `captureSingleScreen`, `cloudEnabled`, `cloudProjectId`, `cpuConfiguration`, `cursorHotspot`, `d3d11ForceExclusiveMode`, `d3d11FullscreenMode`, `d3d12DeviceFilterListAsset`, `d3d9FullscreenMode`, `debugUnloadMode`, `dedicatedServerOptimizations`, `defaultCursor`, `defaultIsFullScreen`, `defaultIsNativeResolution`, `deferSystemGesturesMode`, `disableDepthAndStencilBuffers`, `disableOldInputManagerSupport`, `displayResolutionDialog`, `enableFrameTimingStats`, `enableGamepadInput`, `enableHWStatistics`, `enableNativePlatformBackendsForNewInputSystem`, `enableNewInputSystem`, `enableOpenGLProfilerGPURecorders`, `forceSRGBBlit`, `forceSingleInstance`, `framebufferDepthMemorylessMode`, `fullscreenMode`, `gpuSkinning`, `graphicsJobMode`, `graphicsJobs`, `hdrBitDepth`, `hideHomeButton`, `hmiLoadingImage`, `iPhoneBundleIdentifier`, `ignoreAlphaClear`, `insecureHttpOption`, `invalidatedPatternTexture`, `iosAllowHTTPDownload`, `iosAppInBackgroundBehavior`, `iosShowActivityIndicatorOnLoading`, `iosUseCustomAppBackgroundBehavior`, `isWsaHolographicRemotingEnabled`, `legacyClampBlendShapeWeights`, `loadStoreDebugModeEnabled`, `m_ActiveColorSpace`, `m_ColorGamuts`, `m_HolographicPauseOnTrackingLoss`, `m_HolographicTrackingLossScreen`, `m_MTRendering`, `m_MobileMTRendering`, `m_MobileRenderingPath`, `m_RenderingPath`, `m_ShowUnitySplashLogo`, `m_ShowUnitySplashScreen`, `m_SplashScreenAnimation`, `m_SplashScreenBackgroundAnimationZoom`, `m_SplashScreenBackgroundColor`, `m_SplashScreenBackgroundLandscape`, `m_SplashScreenBackgroundLandscapeAspect`, `m_SplashScreenBackgroundLandscapeUvs`, `m_SplashScreenBackgroundPortrait`, `m_SplashScreenBackgroundPortraitAspect`, `m_SplashScreenBackgroundPortraitUvs`, `m_SplashScreenDrawMode`, `m_SplashScreenLogoAnimationZoom`, `m_SplashScreenLogoStyle`, `m_SplashScreenLogos`, `m_SplashScreenOverlayOpacity`, `m_SplashScreenStyle`, `m_SpriteBatchMaxVertexCount`, `m_SpriteBatchVertexThreshold`, `m_StackTraceTypes`, `m_StereoRenderingPath`, `m_Stereoscopic3D`, `m_SupportedAspectRatios`, `m_UnitySplashLogo`, `m_UseDX11`, `m_VirtualRealitySplashScreen`, `macAppStoreCategory`, `macFullscreenMode`, `macRetinaSupport`, `meshDeformation`, `metalFramebufferOnly`, `metroEnableIndependentInputSource`, `metroEnableLowLatencyPresentationAPI`, `metroInputSource`, `mipStripping`, `mobileMTRenderingBaked`, `muteOtherAudioSources`, `n3dsDisableStereoscopicView`, `n3dsEnableSharedListOpt`, `n3dsEnableVSync`, `numberOfMipsStripped`, `numberOfMipsStrippedPerMipmapLimitGroup`, `organizationId`, `platformRequiresReadableAssets`, `playerDataPath`, `playerMinOpenGLESVersion`, `preloadedAssets`, `preserveFramebufferAlpha`, `productGUID`, `projectId`, `projectName`, `protectGraphicsMemory`, `ps3SplashScreen`, `psp2AcquireBGM`, `psp2PowerMode`, `qualitySettingsNames`, `resetResolutionOnWindowResize`, `resizableWindow`, `resolutionScalingMode`, `singlePassStereoRendering`, `stadiaPresentMode`, `stadiaTargetFramerate`, `stripPhysics`, `submitAnalytics`, `switchAllowGpuScratchShrinking`, `switchGpuScratchPoolGranularity`, `switchGraphicsJobsSyncAfterKick`, `switchMaxWorkerMultiple`, `switchNVNDefaultPoolsGranularity`, `switchNVNGraphicsFirmwareMemory`, `switchNVNMaxPublicSamplerIDCount`, `switchNVNMaxPublicTextureIDCount`, `switchNVNOtherPoolsGranularity`, `switchNVNShaderPoolsGranularity`, `switchQueueCommandMemory`, `switchQueueComputeMemory`, `switchQueueControlMemory`, `targetGlesGraphics`, `targetIOSGraphics`, `targetPixelDensity`, `targetPlatform`, `targetResolution`, `thermalStateCriticalIOSFPS`, `thermalStateSeriousIOSFPS`, `tizenShowActivityIndicatorOnLoading`, `tvOSBundleVersion`, `uiUse16BitDepthBuffer`, `unsupportedMSAAFallback`, `uploadClearedTextureDataAfterCreationFromScript`, `use24BitDepthBuffer`, `useAlphaInDashboard`, `useFlipModelSwapchain`, `useHDRDisplay`, `useOnDemandResources`, `videoMemoryForVertexBuffers`, `virtualRealitySupported`, `virtualTexturingSupportEnabled`, `visibleInBackground`, `visionOSBundleVersion`, `vrSettings`, `vulkanEnableCommandBufferRecycling`, `vulkanEnableLateAcquireNextImage`, `vulkanEnablePreTransform`, `vulkanEnableSetSRGBWrite`, `vulkanNumSwapchainBuffers`, `vulkanUseSWCommandBuffers`, `wiiHio2Usage`, `wiiLoadingScreenBackground`, `wiiLoadingScreenFileName`, `wiiLoadingScreenPeriod`, `wiiLoadingScreenRect`, `wiiLoadingScreenRectPlacement`, `wiiUAllowScreenCapture`, `wiiUControllerCount`, `wiiUGamePadMSAA`, `wiiUSupportsBalanceBoard`, `wiiUSupportsClassicController`, `wiiUSupportsMotionPlus`, `wiiUSupportsNunchuk`, `wiiUSupportsProController`, `wiiUTVResolution`, `windowsGamepadBackendHint`, `wsaTransparentSwapchain`, `xboxEnableAvatar`, `xboxEnableEnableRenderThreadRunsJobs`, `xboxEnableFitness`, `xboxEnableGuest`, `xboxEnableHeadOrientation`, `xboxEnableKinect`, `xboxEnableKinectAutoTracking`, `xboxEnablePIXSampling`, `xboxEnableSpeech`, `xboxOneDisableEsram`, `xboxOneDisableKinectGpuReservation`, `xboxOneEnable7thCore`, `xboxOneEnableTypeOptimization`, `xboxOneLoggingLevel`, `xboxOneMonoLoggingLevel`, `xboxOnePresentImmediateThreshold`, `xboxOneResolution`, `xboxOneSResolution`, `xboxOneXResolution`, `xboxPIXTextureCapture`, `xboxSkinOnGPU`, `xboxSpeechDB` |
| `QualitySettings` | GlobalGameManager | 47 | `Beautiful`, `Fantastic`, `Fast`, `Fastest`, `Good`, `Simple`, `m_CurrentQuality`, `m_DefaultMobileQuality`, `m_DefaultStandaloneQuality`, `m_DefaultWebPlayerQuality`, `m_EditorQuality`, `m_QualitySettings`, `m_StrippedMaximumLODLevel`, `m_TextureMipmapLimitGroupNames` |
| `ResourceManager` | GlobalGameManager | 147 | `m_Container`, `m_DependentAssets` |
| `RuntimeInitializeOnLoadManager` | GlobalGameManager | 300 | `m_AfterAssembliesLoadedMethodExecutionOrders`, `m_AfterAssembliesLoadedUnityMethodExecutionOrders`, `m_AfterMethodExecutionOrders`, `m_AfterUnityMethodExecutionOrders`, `m_AssemblyNames`, `m_BeforeMethodExecutionOrders`, `m_BeforeSplashScreenMethodExecutionOrders`, `m_BeforeSplashScreenUnityMethodExecutionOrders`, `m_BeforeUnityMethodExecutionOrders`, `m_ClassInfos`, `m_ClassMethodInfos`, `m_MethodExecutionOrders`, `m_NamespaceNames`, `m_SubsystemRegistrationMethodExecutionOrders`, `m_SubsystemRegistrationUnityMethodExecutionOrders`, `m_UnityMethodExecutionOrders` |
| `ShaderNameRegistry` | GlobalGameManager | — | `m_PreloadShaders`, `m_Shaders` |
| `StreamingManager` | GlobalGameManager | 1403656975 | — |
| `TagManager` | GlobalGameManager | 78 | `tags`, `Builtin_Layer_0`, `Builtin_Layer_1`, `Builtin_Layer_2`, `Builtin_Layer_3`, `Builtin_Layer_4`, `Builtin_Layer_5`, `Builtin_Layer_6`, `Builtin_Layer_7`, `User_Layer_10`, `User_Layer_11`, `User_Layer_12`, `User_Layer_13`, `User_Layer_14`, `User_Layer_15`, `User_Layer_16`, `User_Layer_17`, `User_Layer_18`, `User_Layer_19`, `User_Layer_20`, `User_Layer_21`, `User_Layer_22`, `User_Layer_23`, `User_Layer_24`, `User_Layer_25`, `User_Layer_26`, `User_Layer_27`, `User_Layer_28`, `User_Layer_29`, `User_Layer_30`, `User_Layer_31`, `User_Layer_8`, `User_Layer_9`, `layers`, `m_RenderingLayers`, `m_SortingLayers` |
| `TimeManager` | GlobalGameManager | 5 | `Fixed_Timestep`, `Maximum_Allowed_Timestep`, `m_TimeScale`, `Maximum_Particle_Timestep` |
| `UnityAdsManager` | GlobalGameManager | 292 | — |
| `UnityAnalyticsManager` | GlobalGameManager | 303 | `m_Enabled`, `m_InitializeOnStartup`, `m_TestConfigUrl`, `m_TestEventUrl`, `m_TestMode` |
| `UnityConnectSettings` | GlobalGameManager | 310 | `UnityAnalyticsSettings`, `UnityPurchasingSettings`, `CrashReportingSettings`, `InsightsSettings`, `PerformanceReportingSettings`, `UnityAdsSettings`, `m_ConfigUrl`, `m_DashboardUrl`, `m_Enabled`, `m_EventOldUrl`, `m_EventUrl`, `m_TestConfigUrl`, `m_TestEventUrl`, `m_TestInitMode`, `m_TestMode` |
| `VFXManager` | GlobalGameManager | 937362698 | `m_CopyBufferShader`, `m_FixedTimeStep`, `m_IndirectShader`, `m_MaxDeltaTime`, `m_RenderPipeSettingsPath`, `m_SortShader`, `m_BatchEmptyLifetime`, `m_CompiledVersion`, `m_EmptyShader`, `m_MaxCapacity`, `m_MaxScrubTime`, `m_PrefixSumShader`, `m_RuntimeResources`, `m_RuntimeVersion`, `m_StripUpdateShader` |
| `LevelGameManager` | GameManager, ABC | 3 | — |
| `HaloManager` | LevelGameManager | 127 | — |
| `LightmapSettings` | LevelGameManager | 157 | `m_Lightmaps`, `m_LightmapsMode`, `m_BakeOnSceneLoad`, `m_BakedColorSpace`, `m_EnlightenSceneMapping`, `m_GISettings`, `m_LightProbes`, `m_LightingSettings`, `m_RuntimeCPUUsage`, `m_ShadowMaskMode`, `m_UseDualLightmapsInForward`, `m_UseShadowmask` |
| `NavMeshSettings` | LevelGameManager | 196 | `m_NavMesh`, `m_NavMeshData` |
| `OcclusionCullingSettings` | LevelGameManager | 29 | `m_OcclusionCullingData`, `m_Portals`, `m_SceneGUID`, `m_StaticRenderers` |
| `RenderSettings` | LevelGameManager | 104 | `m_FlareStrength`, `m_Fog`, `m_FogColor`, `m_FogDensity`, `m_FogMode`, `m_HaloStrength`, `m_HaloTexture`, `m_LinearFogEnd`, `m_LinearFogStart`, `m_SkyboxMaterial`, `m_SpotCookie`, `m_AmbientEquatorColor`, `m_AmbientGroundColor`, `m_AmbientIntensity`, `m_AmbientLight`, `m_AmbientMode`, `m_AmbientProbe`, `m_AmbientProbeInGamma`, `m_AmbientSkyColor`, `m_CustomReflection`, `m_DefaultReflectionMode`, `m_DefaultReflectionResolution`, `m_FlareFadeSpeed`, `m_GeneratedSkyboxReflection`, `m_IndirectSpecularColor`, `m_ReflectionBounces`, `m_ReflectionIntensity`, `m_SubtractiveShadowColor`, `m_Sun`, `m_UseRadianceAmbientProbe` |
| `HierarchyState` | Object | 1026 | `expanded`, `selection`, `scrollposition_x`, `scrollposition_y` |
| `InspectorExpandedState` | Object | 1048 | `m_ExpandedData` |
| `MarshallingTestObject` | Object | — | `m_Prop` |
| `MemorySettings` | Object | — | — |
| `NScreenBridge` | Object | 280 | — |
| `NativeObjectType` | Object | 1977754360 | `m_Inner` |
| `PackedAssets` | Object | 1126 | `m_Contents`, `m_Overhead`, `m_ShortPath`, `m_File` |
| `PlatformModuleSetup` | Object | 877146078 | `modules` |
| `PluginBuildInfo` | Object | 382020655 | `m_EditorPlugins`, `m_RuntimePlugins` |
| `Prefab` | Object | — | `m_RootGameObject`, `m_ContainsMissingSerializeReferenceTypes`, `m_HideFlagsBehaviour`, `m_IsExploded`, `m_IsPrefabAsset`, `m_IsPrefabParent`, `m_Modification`, `m_ParentPrefab`, `m_SourcePrefab` |
| `PrefabInstance` | Object | 1001 | `m_Modification`, `m_RootGameObject`, `m_SourcePrefab` |
| `PresetManager` | Object | 1386491679 | `m_DefaultList`, `m_DefaultPresets` |
| `PropertyModificationsTargetTestObject` | Object | 1111377672 | `m_Array`, `m_Data`, `m_FloatTestValue`, `byte_data`, `m_Bytes`, `m_BytesSize`, `m_Floats` |
| `RenderPassAttachment` | Object | 1571458007 | — |
| `SceneRoots` | Object | — | `m_Roots` |
| `SceneVisibilityState` | Object | 1154873562 | `m_IsolationMode`, `m_MainStageIsolated`, `m_PrefabStageIsolated`, `m_SceneData`, `m_ScenePickingData`, `m_SceneVisibilityData`, `m_SceneVisibilityDataIsolated` |
| `ScenesUsingAssets` | Object | 156483287 | `m_ListOfScenesUsingEachAsset`, `m_ScenesUsingAssets` |
| `SerializableManagedHost` | Object | 1995898324 | `m_Script` |
| `SerializableManagedRefTestClass` | Object | 76251197 | `m_Script` |
| `ShaderContainer` | Object | — | — |
| `SiblingDerived` | Object | 334799969 | — |
| `SpriteAtlasDatabase` | Object | 638013454 | — |
| `TestObjectVectorPairStringBool` | Object | 1628831178 | `m_Map`, `m_String` |
| `TestObjectWithSerializedAnimationCurve` | Object | 478637459 | `m_Curve` |
| `TestObjectWithSerializedArray` | Object | 478637458 | `m_ClampTestValue`, `m_IntegerArray` |
| `TestObjectWithSerializedMapStringBool` | Object | 1981279845 | `m_Map`, `m_String` |
| `TestObjectWithSerializedMapStringNonAlignedStruct` | Object | 342846651 | `m_Map`, `m_String` |
| `TestObjectWithSpecialLayoutOne` | Object | 293259124 | `differentLayout`, `sameLayout` |
| `TestObjectWithSpecialLayoutTwo` | Object | 1392443030 | `differentLayout`, `sameLayout` |
| `TilemapEditorUserSettings` | Object | — | `m_FocusMode`, `m_LastUsedPalette` |
| `VersionControlSettings` | Object | 890905787 | `m_Mode`, `m_CollabEditorSettings`, `m_TrackPackagesOutsideProject` |
| `VideoBuildInfo` | Object | — | `m_IsVideoModuleDisabled`, `m_VideoClipCount` |
| `AABB` | object | — | `m_Center`, `m_Extent` |
| `AddedComponent` | object | — | `addedObject`, `insertIndex`, `targetCorrespondingSourceObject` |
| `AddedGameObject` | object | — | `addedObject`, `insertIndex`, `targetCorrespondingSourceObject` |
| `AndroidDeviceFilterData` | object | — | `androidOsVersionString`, `brandName`, `deviceName`, `driverVersionString`, `productName`, `vendorName`, `vulkanApiVersionString` |
| `AnimationClipBindingConstant` | object | — | `genericBindings`, `pptrCurveMapping` |
| `AnimationClipOverride` | object | — | `m_OriginalClip`, `m_OverrideClip` |
| `AnimationCurve` | object | — | `m_Curve`, `m_PostInfinity`, `m_PreInfinity`, `m_RotationOrder` |
| `AnimationEvent` | object | — | `data`, `floatParameter`, `functionName`, `intParameter`, `messageOptions`, `objectReferenceParameter`, `time` |
| `AnimatorCondition` | object | — | `m_ConditionEvent`, `m_ConditionMode`, `m_EventTreshold` |
| `Annotation` | object | — | `m_ClassID`, `m_Flags`, `m_GizmoEnabled`, `m_IconEnabled`, `m_ScriptClass` |
| `ArticulationDrive` | object | — | `damping`, `forceLimit`, `lowerLimit`, `stiffness`, `target`, `targetVelocity`, `upperLimit`, `driveType` |
| `AspectRatios` | object | — | `Others`, `x16_10`, `x16_9`, `x4_3`, `x5_4` |
| `AssemblyJsonAsset` | TextAsset | — | `m_Name`, `m_Script`, `m_PathName` |
| `AssemblyJsonImporter` | AssetImporter | — | `m_AssetBundleName`, `m_AssetBundleVariant`, `m_Name`, `m_UserData`, `m_ExternalObjects` |
| `Asset` | object | — | `children`, `labels`, `mainRepresentation`, `parent`, `representations`, `type`, `assetBundleIndex`, `digest`, `guidOfPathLocationDependencies`, `hash`, `hashOfImportedAssetDependencies`, `hashOfSourceAssetDependencies`, `importerClassId`, `importerVersionHash`, `metaModificationDate_0_`, `metaModificationDate_1_`, `modificationDate_0_`, `modificationDate_1_`, `scriptedImporterClassID` |
| `AssetBundleFullName` | object | — | `m_AssetBundleName`, `m_AssetBundleVariant` |
| `AssetBundleInfo` | object | — | `AssetBundleDependencies`, `AssetBundleHash` |
| `AssetBundleScriptInfo` | object | — | `assemblyName`, `className`, `hash`, `nameSpace` |
| `AssetDatabase` | Object | — | `m_Assets`, `m_AssetBundleNames`, `m_AssetTimeStamps`, `m_Metrics`, `m_UnityShadersVersion`, `m_lastValidVersionHashes` |
| `AssetDatabaseMetrics` | object | — | `totalAssetCount`, `nonProAssetCount`, `nonProAssetsCreatedAfterProLicense` |
| `AssetImporterHashKey` | object | — | `ScriptClass`, `type` |
| `AssetImporterLog` | NamedObject | 41386430 | `m_Logs`, `m_Name` |
| `AssetImporter_ImportError` | object | — | `error`, `file`, `line`, `mode`, `object` |
| `AssetInfo` | object | — | `asset`, `preloadIndex`, `preloadSize` |
| `AssetLabels` | object | — | `m_Labels` |
| `AssetStats` | object | — | `objectCount`, `resourceCount`, `size`, `sourceAssetGUID`, `sourceAssetPath` |
| `AssetTimeStamp` | object | — | `metaModificationDate_0_`, `metaModificationDate_1_`, `modificationDate_0_`, `modificationDate_1_` |
| `AttachmentIndexArray` | object | — | `activeAttachments`, `attachments` |
| `AttachmentInfo` | object | — | `format`, `needsResolve`, `canMultiview`, `loadAction`, `sampleCount`, `storeAction` |
| `AudioImporterOutput` | object | — | `editorOutputContainerFormat`, `editorOutputSettings`, `outputContainerFormat`, `outputSettings`, `playerResource` |
| `AudioMixerConstant` | object | — | `effectGUIDs`, `effects`, `exposedParameterIndices`, `exposedParameterNames`, `groupGUIDs`, `groupNameBuffer`, `groups`, `numSideChainBuffers`, `pluginEffectNameBuffer`, `snapshotGUIDs`, `snapshotNameBuffer`, `snapshots`, `groupConnections` |
| `AudioMixerLiveUpdateBool` | ABC | 100009 | — |
| `AudioMixerLiveUpdateFloat` | ABC | 100008 | — |
| `AutoOffMeshLinkData` | object | — | `m_Area`, `m_End`, `m_LinkDirection`, `m_LinkType`, `m_Radius`, `m_Start` |
| `AvatarBodyMask` | NamedObject | — | `m_Mask`, `m_Name` |
| `AvatarConstant` | object | — | `m_Human`, `m_HumanSkeletonIndexArray`, `m_RootMotionBoneIndex`, `m_RootMotionBoneX`, `m_AvatarSkeleton`, `m_AvatarSkeletonPose`, `m_DefaultPose`, `m_HumanSkeletonReverseIndexArray`, `m_RootMotionSkeleton`, `m_RootMotionSkeletonIndexArray`, `m_RootMotionSkeletonPose`, `m_Skeleton`, `m_SkeletonNameIDArray`, `m_SkeletonPose` |
| `AvatarSkeletonMaskElement` | object | — | `path`, `weight` |
| `Axes` | object | — | `m_Length`, `m_Limit`, `m_PostQ`, `m_PreQ`, `m_Sgn`, `m_Type` |
| `BitField` | object | — | `m_Bits` |
| `Blend1dDataConstant` | object | — | `m_ChildThresholdArray` |
| `Blend2dDataConstant` | object | — | `m_ChildMagnitudeArray`, `m_ChildNeighborListArray`, `m_ChildPairAvgMagInvArray`, `m_ChildPairVectorArray`, `m_ChildPositionArray`, `m_ChildThresholdArray` |
| `BlendDirectDataConstant` | object | — | `m_ChildBlendEventIDArray`, `m_NormalizedBlendValues` |
| `BlendShapeData` | object | — | `channels`, `fullWeights`, `shapes`, `vertices` |
| `BlendShapeVertex` | object | — | `index`, `normal`, `tangent`, `vertex` |
| `BlendTreeConstant` | object | — | `m_NodeArray`, `m_BlendEventArrayConstant` |
| `BlendTreeNodeConstant` | object | — | `m_BlendEventID`, `m_ChildIndices`, `m_ClipID`, `m_Duration`, `m_Blend1dData`, `m_Blend2dData`, `m_BlendDirectData`, `m_BlendEventYID`, `m_BlendType`, `m_ChildThresholdArray`, `m_ClipIndex`, `m_CycleOffset`, `m_Mirror` |
| `BoneInfluence` | object | — | `boneIndex_0_`, `boneIndex_1_`, `boneIndex_2_`, `boneIndex_3_`, `weight_0_`, `weight_1_`, `weight_2_`, `weight_3_` |
| `BoneWeights4` | object | — | `boneIndex_0_`, `boneIndex_1_`, `boneIndex_2_`, `boneIndex_3_`, `weight_0_`, `weight_1_`, `weight_2_`, `weight_3_` |
| `BranchWindLevel` | object | — | `m_afBend_0`, `m_afBend_1`, `m_afBend_10`, `m_afBend_11`, `m_afBend_12`, `m_afBend_13`, `m_afBend_14`, `m_afBend_15`, `m_afBend_16`, `m_afBend_17`, `m_afBend_18`, `m_afBend_19`, `m_afBend_2`, `m_afBend_3`, `m_afBend_4`, `m_afBend_5`, `m_afBend_6`, `m_afBend_7`, `m_afBend_8`, `m_afBend_9`, `m_afFlexibility_0`, `m_afFlexibility_1`, `m_afFlexibility_10`, `m_afFlexibility_11`, `m_afFlexibility_12`, `m_afFlexibility_13`, `m_afFlexibility_14`, `m_afFlexibility_15`, `m_afFlexibility_16`, `m_afFlexibility_17`, `m_afFlexibility_18`, `m_afFlexibility_19`, `m_afFlexibility_2`, `m_afFlexibility_3`, `m_afFlexibility_4`, `m_afFlexibility_5`, `m_afFlexibility_6`, `m_afFlexibility_7`, `m_afFlexibility_8`, `m_afFlexibility_9`, `m_afOscillation_0`, `m_afOscillation_1`, `m_afOscillation_10`, `m_afOscillation_11`, `m_afOscillation_12`, `m_afOscillation_13`, `m_afOscillation_14`, `m_afOscillation_15`, `m_afOscillation_16`, `m_afOscillation_17`, `m_afOscillation_18`, `m_afOscillation_19`, `m_afOscillation_2`, `m_afOscillation_3`, `m_afOscillation_4`, `m_afOscillation_5`, `m_afOscillation_6`, `m_afOscillation_7`, `m_afOscillation_8`, `m_afOscillation_9`, `m_afSpeed_0`, `m_afSpeed_1`, `m_afSpeed_10`, `m_afSpeed_11`, `m_afSpeed_12`, `m_afSpeed_13`, `m_afSpeed_14`, `m_afSpeed_15`, `m_afSpeed_16`, `m_afSpeed_17`, `m_afSpeed_18`, `m_afSpeed_19`, `m_afSpeed_2`, `m_afSpeed_3`, `m_afSpeed_4`, `m_afSpeed_5`, `m_afSpeed_6`, `m_afSpeed_7`, `m_afSpeed_8`, `m_afSpeed_9`, `m_afTurbulence_0`, `m_afTurbulence_1`, `m_afTurbulence_10`, `m_afTurbulence_11`, `m_afTurbulence_12`, `m_afTurbulence_13`, `m_afTurbulence_14`, `m_afTurbulence_15`, `m_afTurbulence_16`, `m_afTurbulence_17`, `m_afTurbulence_18`, `m_afTurbulence_19`, `m_afTurbulence_2`, `m_afTurbulence_3`, `m_afTurbulence_4`, `m_afTurbulence_5`, `m_afTurbulence_6`, `m_afTurbulence_7`, `m_afTurbulence_8`, `m_afTurbulence_9`, `m_fIndependence` |
| `BufferBinding` | object | — | `m_Index`, `m_NameIndex`, `m_ArraySize` |
| `BuildReportFile` | object | — | `id`, `path`, `role`, `totalSize`, `flags` |
| `BuildReportPackedAssetInfo` | object | — | `classID`, `fileID`, `packedSize`, `sourceAssetGUID`, `buildTimeAssetPath`, `offset` |
| `BuildReportScenesUsingAsset` | object | — | `assetPath`, `scenePaths` |
| `BuildStepInfo` | object | — | `messages`, `stepName`, `depth`, `duration`, `durationTicks` |
| `BuildStepMessage` | object | — | `content`, `type` |
| `BuildSummary` | object | — | `assetBundleOptions`, `crc`, `options`, `outputPath`, `platformName`, `totalErrors`, `totalSize`, `totalWarnings`, `buildContentOptions`, `buildGUID`, `buildManifestHash`, `buildResult`, `buildStartTime`, `buildType`, `dataPath`, `multiProcessEnabled`, `name`, `platformGroupName`, `subtarget`, `success`, `totalTimeMS`, `totalTimeTicks` |
| `BuildTargetSettings` | object | — | `m_BuildTarget`, `m_TextureFormat`, `m_AllowsAlphaSplitting`, `m_CompressionQuality`, `m_LoadingBehavior`, `m_MaxTextureSize`, `m_TextureHeight`, `m_TextureWidth` |
| `BuildTextureStackReference` | object | — | `groupName`, `itemName` |
| `BuiltAssetBundleInfo` | object | — | `bundleArchiveFile`, `bundleName`, `packagedFileIndices` |
| `BuiltinShaderSettings` | object | — | `m_Mode`, `m_Shader` |
| `CGProgram` | TextAsset | 109 | `m_Name`, `m_Script`, `m_PathName` |
| `CachedAssetMetaData` | object | — | `guid`, `originalChangeset`, `originalDigest`, `originalName`, `originalParent`, `pathName` |
| `Channel` | object | — | `attributeName`, `byteOffset`, `curve` |
| `ChannelInfo` | object | — | `dimension`, `format`, `offset`, `stream` |
| `CharacterInfo` | object | — | `index`, `uv`, `vert`, `advance`, `flipped`, `width` |
| `Child` | object | — | `m_IsAnim`, `m_Motion`, `m_Threshold`, `m_TimeScale`, `m_CycleOffset`, `m_Mirror`, `m_Position` |
| `ChildAnimatorState` | object | — | `m_Position`, `m_State` |
| `ChildAnimatorStateMachine` | object | — | `m_Position`, `m_StateMachine` |
| `ChildMotion` | object | — | `m_CycleOffset`, `m_DirectBlendParameter`, `m_Mirror`, `m_Motion`, `m_Position`, `m_Threshold`, `m_TimeScale` |
| `ClampVelocityModule` | object | — | `dampen`, `enabled`, `magnitude`, `separateAxis`, `x`, `y`, `z`, `drag`, `inWorldSpace`, `multiplyDragByParticleSize`, `multiplyDragByParticleVelocity` |
| `ClassInfo` | object | — | `m_AssemblyNameIndex`, `m_ClassName`, `m_IsUnityClass`, `m_MethodIndex`, `m_NamespaceIndex`, `m_NumOfMethods`, `m_NamespaceName` |
| `ClassMethodInfo` | object | — | `m_ClassIndex`, `m_MethodName`, `m_OrderNumber` |
| `Clip` | object | — | `m_DenseClip`, `m_StreamedClip`, `m_Binding`, `m_ConstantClip` |
| `ClipAnimationInfo` | object | — | `firstFrame`, `lastFrame`, `loop`, `name`, `wrapMode`, `additiveReferencePoseFrame`, `bodyMask`, `curves`, `cycleOffset`, `events`, `hasAdditiveReferencePose`, `heightFromFeet`, `internalID`, `keepAdditionalBonesAnimation`, `keepOriginalOrientation`, `keepOriginalPositionXZ`, `keepOriginalPositionY`, `level`, `loopBlend`, `loopBlendOrientation`, `loopBlendPositionXZ`, `loopBlendPositionY`, `loopTime`, `maskSource`, `maskType`, `mirror`, `orientationOffsetY`, `skeletonMaskElements`, `takeName`, `transformMask` |
| `ClipAnimationInfoCurve` | object | — | `curve`, `name` |
| `ClipMuscleConstant` | object | — | `m_AverageAngularSpeed`, `m_AverageSpeed`, `m_Clip`, `m_CycleOffset`, `m_DeltaPose`, `m_HeightFromFeet`, `m_IndexArray`, `m_KeepOriginalOrientation`, `m_KeepOriginalPositionXZ`, `m_KeepOriginalPositionY`, `m_LeftFootStartX`, `m_Level`, `m_LoopBlend`, `m_LoopBlendOrientation`, `m_LoopBlendPositionXZ`, `m_LoopBlendPositionY`, `m_Mirror`, `m_OrientationOffsetY`, `m_RightFootStartX`, `m_StartTime`, `m_StartX`, `m_StopTime`, `m_ValueArrayDelta`, `m_AdditionalCurveIndexArray`, `m_LoopTime`, `m_MotionStartX`, `m_MotionStopX`, `m_StartAtOrigin`, `m_StopX`, `m_ValueArrayReferencePose` |
| `ClothAttachment` | object | — | `m_Collider`, `m_Tearable`, `m_TwoWayInteraction` |
| `ClothConstrainCoefficients` | object | — | `collisionSphereDistance`, `maxDistance`, `collisionSphereRadius`, `maxDistanceBias` |
| `ClothSphereColliderPair` | object | — | `first`, `second` |
| `ClusterInput` | object | — | `m_DeviceName`, `m_Index`, `m_Name`, `m_ServerUrl`, `m_Type` |
| `CollabEditorSettings` | object | — | `inProgressEnabled` |
| `Collision` | ABC | 100004 | — |
| `Collision2D` | ABC | 100007 | — |
| `CollisionModule` | object | — | `enabled`, `minKillSpeed`, `type`, `bounce`, `colliderForce`, `collidesWith`, `collidesWithDynamic`, `collisionMessages`, `collisionMode`, `dampen`, `energyLossOnCollision`, `interiorCollisions`, `m_Bounce`, `m_Dampen`, `m_EnergyLossOnCollision`, `m_Planes`, `maxCollisionShapes`, `maxKillSpeed`, `multiplyColliderForceByCollisionAngle`, `multiplyColliderForceByParticleSize`, `multiplyColliderForceByParticleSpeed`, `particleRadius`, `plane0`, `plane1`, `plane2`, `plane3`, `plane4`, `plane5`, `quality`, `radiusScale`, `voxelSize` |
| `ColorBySpeedModule` | object | — | `enabled`, `gradient`, `range` |
| `ColorModule` | object | — | `enabled`, `gradient` |
| `ComponentPair` | object | — | `component` |
| `CompressedAnimationCurve` | object | — | `m_Path`, `m_PostInfinity`, `m_PreInfinity`, `m_Slopes`, `m_Times`, `m_Values` |
| `CompressedMesh` | object | — | `m_BoneIndices`, `m_NormalSigns`, `m_Normals`, `m_TangentSigns`, `m_Tangents`, `m_Triangles`, `m_UV`, `m_Vertices`, `m_Weights`, `m_BindPoses`, `m_Colors`, `m_FloatColors`, `m_UVInfo` |
| `ComputeBufferCounter` | object | — | `bindpoint`, `offset` |
| `ComputeShaderBuiltinSampler` | object | — | `bindPoint`, `sampler` |
| `ComputeShaderCB` | object | — | `byteSize`, `name`, `params` |
| `ComputeShaderKernel` | object | — | `builtinSamplers`, `cbs`, `code`, `inBuffers`, `outBuffers`, `textures`, `cbVariantIndices`, `name`, `requirements`, `threadGroupSize` |
| `ComputeShaderKernelParent` | object | — | `name`, `dynamicKeywords`, `globalKeywords`, `localKeywords`, `uniqueVariants`, `validKeywords`, `variantIndices`, `variantMap` |
| `ComputeShaderParam` | object | — | `arraySize`, `colCount`, `name`, `offset`, `rowCount`, `type` |
| `ComputeShaderPlatformVariant` | object | — | `constantBuffers`, `kernels`, `resourcesResolved`, `targetLevel`, `targetRenderer` |
| `ComputeShaderResource` | object | — | `bindPoint`, `name`, `counter`, `generatedName`, `resType`, `samplerBindPoint`, `secondaryBindPoint`, `texDimension` |
| `ComputeShaderVariant` | object | — | `constantBuffers`, `kernels`, `targetLevel`, `targetRenderer`, `resourcesResolved` |
| `Condition` | object | — | `m_ConditionEvent`, `m_ConditionMode`, `m_EventTreshold`, `m_ExitTime` |
| `ConditionConstant` | object | — | `m_ConditionMode`, `m_EventID`, `m_EventThreshold`, `m_ExitTime` |
| `ConfigSetting` | object | — | `flags`, `value` |
| `ConstantBuffer` | object | — | `m_MatrixParams`, `m_NameIndex`, `m_Size`, `m_VectorParams`, `m_IsPartialCB`, `m_StructParams` |
| `ConstantClip` | object | — | `data` |
| `ConstraintSource` | object | — | `sourceTransform`, `weight` |
| `ControllerConstant` | object | — | `m_DefaultValues`, `m_StateMachineArray`, `m_Values`, `m_HumanLayerArray`, `m_LayerArray` |
| `CrashReportingSettings` | object | — | `m_EventUrl`, `m_EnableCloudDiagnosticsReporting`, `m_Enabled`, `m_LogBufferSize`, `m_NativeEventUrl` |
| `CustomDataModule` | object | — | `color0`, `color1`, `enabled`, `mode0`, `mode1`, `vector0_0`, `vector0_1`, `vector0_2`, `vector0_3`, `vector1_0`, `vector1_1`, `vector1_2`, `vector1_3`, `vectorComponentCount0`, `vectorComponentCount1` |
| `D3D12DeviceFilterData` | object | — | `deviceName`, `deviceType`, `driverVersion`, `driverVersionComparator`, `featureLevel`, `featureLevelComparator`, `graphicsMemory`, `graphicsMemoryComparator`, `processorCount`, `processorCountComparator`, `vendorName` |
| `D3D12GraphicsJobsDeviceFilterData` | object | — | `filter`, `preferredMode` |
| `DataTemplate` | NamedObject | — | `m_Father`, `m_IsDataTemplate`, `m_LastMergeIdentifier`, `m_Name`, `m_Objects` |
| `DateTime` | object | — | `ticks` |
| `DefaultPreset` | object | — | `m_Preset`, `m_Disabled`, `m_Filter` |
| `DefaultPresetList` | object | — | `defaultPresets`, `type` |
| `DeletedItem` | object | — | `changeset`, `digest`, `fullPath`, `guid`, `parent`, `type` |
| `DenseClip` | object | — | `m_BeginTime`, `m_CurveCount`, `m_FrameCount`, `m_SampleArray`, `m_SampleRate` |
| `DetailDatabase` | object | — | `WavingGrassTint`, `m_DetailPrototypes`, `m_PatchCount`, `m_PatchSamples`, `m_Patches`, `m_PreloadTextureAtlasData`, `m_TreeInstances`, `m_TreePrototypes`, `m_WavingGrassAmount`, `m_WavingGrassSpeed`, `m_WavingGrassStrength`, `m_DefaultShaders_0_`, `m_DefaultShaders_1_`, `m_DefaultShaders_2_`, `m_DetailBillboardShader`, `m_DetailMeshGrassShader`, `m_DetailMeshLitShader`, `m_DetailScatterMode`, `m_RandomRotations` |
| `DetailPatch` | object | — | `layerIndices`, `bounds`, `coverage`, `numberOfObjects` |
| `DetailPrototype` | object | — | `dryColor`, `healthyColor`, `maxHeight`, `maxWidth`, `minHeight`, `minWidth`, `noiseSpread`, `prototype`, `prototypeTexture`, `renderMode`, `usePrototypeMesh`, `alignToGround`, `bendFactor`, `density`, `holeTestRadius`, `lightmapFactor`, `noiseSeed`, `positionJitter`, `positionOrderliness`, `targetCoverage`, `useDensityScaling`, `useInstancing` |
| `DeviceNone` | object | — | — |
| `DirectorGenericBinding` | object | — | `key`, `value` |
| `DirectorPlayer` | Behaviour | — | `m_GameObject` |
| `EffectConstant` | object | — | `bypass`, `groupConstantIndex`, `parameterIndices`, `prevEffectIndex`, `sendTargetEffectIndex`, `type`, `wetMixLevelIndex` |
| `EmbeddedNativeType` | object | — | `m_FloatArray`, `m_String` |
| `EmissionModule` | object | — | `enabled`, `m_BurstCount`, `cnt0`, `cnt1`, `cnt2`, `cnt3`, `cntmax0`, `cntmax1`, `cntmax2`, `cntmax3`, `m_Bursts`, `m_Type`, `rate`, `rateOverDistance`, `rateOverTime`, `time0`, `time1`, `time2`, `time3` |
| `EnlightenRendererInformation` | object | — | `dynamicLightmapSTInSystem`, `instanceHash`, `renderer`, `systemId` |
| `EnlightenSceneMapping` | object | — | `m_Renderers`, `m_SystemAtlases`, `m_Systems`, `m_TerrainChunks`, `m_Probesets` |
| `EnlightenSystemAtlasInformation` | object | — | `atlasHash`, `atlasSize`, `firstSystemId` |
| `EnlightenSystemInformation` | object | — | `atlasIndex`, `atlasOffsetX`, `atlasOffsetY`, `inputSystemHash`, `radiositySystemHash`, `rendererIndex`, `rendererSize` |
| `EnlightenTerrainChunksInformation` | object | — | `firstSystemId`, `numChunksInX`, `numChunksInY` |
| `EntityId` | object | — | — |
| `Error` | object | — | `filePath`, `message`, `severity`, `startChar`, `startLine` |
| `ExpandedData` | object | — | `m_ClassID`, `m_ExpandedProperties`, `m_InspectorExpanded`, `m_ScriptClass` |
| `ExposedReferenceTable` | object | — | `m_References` |
| `Expression` | object | — | `data_0_`, `data_1_`, `data_2_`, `data_3_`, `op`, `valueIndex` |
| `ExtensionPropertyValue` | object | — | `extensionName`, `pluginName`, `propertyName`, `propertyValue` |
| `ExternalForcesModule` | object | — | `enabled`, `influenceFilter`, `influenceList`, `influenceMask`, `multiplier`, `multiplierCurve` |
| `FalloffTable` | object | — | `m_Table_0_`, `m_Table_10_`, `m_Table_11_`, `m_Table_12_`, `m_Table_1_`, `m_Table_2_`, `m_Table_3_`, `m_Table_4_`, `m_Table_5_`, `m_Table_6_`, `m_Table_7_`, `m_Table_8_`, `m_Table_9_` |
| `FastPropertyName` | object | — | `name` |
| `FlareElement` | object | — | `m_Color`, `m_Fade`, `m_ImageIndex`, `m_Position`, `m_Rotate`, `m_Size`, `m_UseLightColor`, `m_Zoom` |
| `FloatCurve` | object | — | `attribute`, `classID`, `curve`, `path`, `script`, `flags` |
| `ForceModule` | object | — | `enabled`, `inWorldSpace`, `randomizePerFrame`, `x`, `y`, `z` |
| `GISettings` | object | — | `m_AlbedoBoost`, `m_BounceScale`, `m_EnableBakedLightmaps`, `m_EnableRealtimeLightmaps`, `m_EnvironmentLightingMode`, `m_IndirectOutputScale`, `m_TemporalCoherenceThreshold` |
| `GLTextureSettings` | object | — | `m_Aniso`, `m_FilterMode`, `m_MipBias`, `m_WrapMode`, `m_WrapU`, `m_WrapV`, `m_WrapW` |
| `GUID` | object | — | `data_0_`, `data_1_`, `data_2_`, `data_3_` |
| `GenericBinding` | object | — | `attribute`, `customType`, `isPPtrCurve`, `path`, `script`, `classID`, `isIntCurve`, `isSerializeReferenceCurve`, `typeID` |
| `GfxBlendState` | object | — | `alphaToMask`, `rt`, `separateMRTBlend` |
| `GfxDepthState` | object | — | `depthFunc`, `depthWrite` |
| `GfxRasterState` | object | — | `conservative`, `cullMode`, `depthBias`, `depthClip`, `slopeScaledDepthBias` |
| `GfxRenderTargetBlendState` | object | — | `blendOp`, `blendOpAlpha`, `dstBlend`, `dstBlendAlpha`, `srcBlend`, `srcBlendAlpha`, `writeMask` |
| `GfxStencilState` | object | — | `padding`, `readMask`, `stencilEnable`, `stencilFailOpBack`, `stencilFailOpFront`, `stencilFuncBack`, `stencilFuncFront`, `stencilPassOpBack`, `stencilPassOpFront`, `stencilZFailOpBack`, `stencilZFailOpFront`, `writeMask` |
| `Google` | object | — | `depthFormat`, `enableTransitionView`, `enableVideoLayer`, `maximumSupportedHeadTracking`, `minimumSupportedHeadTracking`, `useProtectedVideoMemory`, `useSustainedPerformanceMode` |
| `Gradient` | object | — | `atime0`, `atime1`, `atime2`, `atime3`, `atime4`, `atime5`, `atime6`, `atime7`, `ctime0`, `ctime1`, `ctime2`, `ctime3`, `ctime4`, `ctime5`, `ctime6`, `ctime7`, `key0`, `key1`, `key2`, `key3`, `key4`, `key5`, `key6`, `key7`, `m_ColorSpace`, `m_Color_0_`, `m_Color_1_`, `m_Color_2_`, `m_Color_3_`, `m_Color_4_`, `m_Mode`, `m_NumAlphaKeys`, `m_NumColorKeys` |
| `GradientNEW` | object | — | `atime0`, `atime1`, `atime2`, `atime3`, `atime4`, `atime5`, `atime6`, `atime7`, `ctime0`, `ctime1`, `ctime2`, `ctime3`, `ctime4`, `ctime5`, `ctime6`, `ctime7`, `key0`, `key1`, `key2`, `key3`, `key4`, `key5`, `key6`, `key7`, `m_NumAlphaKeys`, `m_NumColorKeys` |
| `GraphicsStateInfo` | object | — | `appBackface`, `depthBias`, `forceCullMode`, `invertProjectionMatrix`, `renderPass`, `renderState`, `slopeDepthBias`, `subPassIndex`, `topology`, `userBackface`, `vertexLayout`, `wireframe`, `baseShadingRate`, `nonfilteringSamplerBindings`, `shadingRateCombinerFragment`, `shadingRateCombinerPrimitive`, `unfilterableTextureBindings` |
| `GroupConnection` | object | — | `sendEffectIndex`, `sourceGroupIndex`, `targetGroupIndex` |
| `GroupConstant` | object | — | `bypassEffects`, `mute`, `parentConstantIndex`, `pitchIndex`, `solo`, `volumeIndex`, `sendIndex` |
| `Hand` | object | — | `m_HandBoneIndex` |
| `HandPose` | object | — | `m_CloseOpen`, `m_DoFArray`, `m_Grab`, `m_GrabX`, `m_InOut`, `m_Override` |
| `Handle` | object | — | `m_ID`, `m_ParentHumanIndex`, `m_X` |
| `Hash128` | object | — | `bytes_0_`, `bytes_10_`, `bytes_11_`, `bytes_12_`, `bytes_13_`, `bytes_14_`, `bytes_15_`, `bytes_1_`, `bytes_2_`, `bytes_3_`, `bytes_4_`, `bytes_5_`, `bytes_6_`, `bytes_7_`, `bytes_8_`, `bytes_9_` |
| `HeightMeshBVNode` | object | — | `i`, `max`, `min`, `n` |
| `HeightMeshData` | object | — | `m_Bounds`, `m_Indices`, `m_Nodes`, `m_Vertices` |
| `Heightmap` | object | — | `m_Heights`, `m_Levels`, `m_MinMaxPatchHeights`, `m_PrecomputedError`, `m_Scale`, `m_DefaultPhysicMaterial`, `m_EnableHolesTextureCompression`, `m_EnableSurfaceMaskTextureCompression`, `m_Height`, `m_Holes`, `m_HolesLOD`, `m_Resolution`, `m_SurfaceMask`, `m_SurfaceMaskLOD`, `m_Thickness`, `m_Width` |
| `HeightmapData` | object | — | `terrainData`, `isRotated`, `position`, `surfaceToTerrain` |
| `HierarchicalSceneData` | object | — | `m_SceneGUID` |
| `HoloLens` | object | — | `depthFormat`, `depthBufferSharingEnabled` |
| `Human` | object | — | `m_ArmStretch`, `m_ArmTwist`, `m_FeetSpacing`, `m_ForeArmTwist`, `m_HasLeftHand`, `m_HasRightHand`, `m_HumanBoneIndex`, `m_HumanBoneMass`, `m_LeftHand`, `m_LegStretch`, `m_LegTwist`, `m_RightHand`, `m_RootX`, `m_Scale`, `m_Skeleton`, `m_SkeletonPose`, `m_UpperLegTwist`, `m_ColliderArray`, `m_ColliderIndex`, `m_Handles`, `m_HasTDoF` |
| `HumanBone` | object | — | `m_BoneName`, `m_HumanName`, `m_Limit` |
| `HumanDescription` | object | — | `m_ArmStretch`, `m_ArmTwist`, `m_FeetSpacing`, `m_ForeArmTwist`, `m_Human`, `m_LegStretch`, `m_LegTwist`, `m_RootMotionBoneName`, `m_Skeleton`, `m_UpperLegTwist`, `m_GlobalScale`, `m_Handles`, `m_HasExtraRoot`, `m_HasTranslationDoF`, `m_RootMotionBoneRotation`, `m_SkeletonHasParents` |
| `HumanGoal` | object | — | `m_WeightR`, `m_WeightT`, `m_X`, `m_HintT`, `m_HintWeightT` |
| `HumanHandle` | object | — | `m_BoneName`, `m_LookAt`, `m_Name`, `m_Position`, `m_Rotation`, `m_Scale` |
| `HumanLayerConstant` | object | — | `m_Binding`, `m_BodyMask`, `m_IKPass`, `m_LayerBlendingMode`, `m_SkeletonMask`, `m_StateMachineIndex`, `m_StateMachineMotionSetIndex`, `m_DefaultWeight`, `m_SyncedLayerAffectsTiming` |
| `HumanPose` | object | — | `m_DoFArray`, `m_GoalArray`, `m_LeftHandPose`, `m_LookAtPosition`, `m_LookAtWeight`, `m_RightHandPose`, `m_RootX`, `m_TDoFArray` |
| `HumanPoseMask` | object | — | `word0`, `word1`, `word2` |
| `Image` | object | — | `image_data`, `m_Format`, `m_Height`, `m_RowBytes`, `m_Width` |
| `ImportLog_ImportLogEntry` | object | — | `file`, `line`, `message`, `mode`, `object` |
| `InheritVelocityModule` | object | — | `enabled`, `m_Curve`, `m_Mode` |
| `InitialModule` | object | — | `enabled`, `gravityModifier`, `maxNumParticles`, `startColor`, `startLifetime`, `startRotation`, `startSize`, `startSpeed`, `customEmitterVelocity`, `gravitySource`, `inheritVelocity`, `randomizeRotationDirection`, `rotation3D`, `size3D`, `startRotationX`, `startRotationY`, `startSizeY`, `startSizeZ` |
| `InputAxis` | object | — | `altNegativeButton`, `altPositiveButton`, `axis`, `dead`, `descriptiveName`, `descriptiveNegativeName`, `gravity`, `invert`, `joyNum`, `m_Name`, `negativeButton`, `positiveButton`, `sensitivity`, `snap`, `type` |
| `InputImportSettings` | object | — | `name`, `alphaSource`, `aniso`, `filterMode`, `value`, `wrapMode` |
| `InsightsSettings` | object | — | `m_Enabled`, `m_EngineDiagnosticsEnabled`, `m_EventUrl`, `m_StackTraceUrl` |
| `IntPoint` | object | — | `X`, `Y` |
| `Item` | object | — | `changeFlags`, `changeset`, `digest`, `downloadResolution`, `guid`, `markedForRemoval`, `name`, `nameConflictResolution`, `oldVersion`, `origin`, `parent`, `parentFolderID`, `type` |
| `JointAngleLimit2D` | object | — | `m_LowerAngle`, `m_UpperAngle` |
| `JointAngleLimits2D` | object | — | `m_LowerAngle`, `m_UpperAngle` |
| `JointDrive` | object | — | `maximumForce`, `positionDamper`, `positionSpring`, `mode`, `useAcceleration` |
| `JointLimits` | object | — | `max`, `min`, `bounceMinVelocity`, `bounciness`, `contactDistance`, `maxBounce`, `minBounce` |
| `JointMotor` | object | — | `force`, `freeSpin`, `targetVelocity` |
| `JointMotor2D` | object | — | `m_MaximumMotorForce`, `m_MotorSpeed` |
| `JointSpring` | object | — | `damper`, `spring`, `targetPosition` |
| `JointSuspension2D` | object | — | `m_Angle`, `m_DampingRatio`, `m_Frequency` |
| `JointTranslationLimits2D` | object | — | `m_LowerTranslation`, `m_UpperTranslation` |
| `Keyframe` | object | — | `inSlope`, `outSlope`, `time`, `value`, `inWeight`, `outWeight`, `weightedMode` |
| `LOD` | object | — | `renderers`, `screenRelativeHeight`, `fadeMode`, `fadeTransitionWidth` |
| `LODRenderer` | object | — | `renderer` |
| `LayerConstant` | object | — | `m_Binding`, `m_BodyMask`, `m_DefaultWeight`, `m_IKPass`, `m_LayerBlendingMode`, `m_SkeletonMask`, `m_StateMachineIndex`, `m_SyncedLayerAffectsTiming`, `m_StateMachineMotionSetIndex`, `m_StateMachineSynchronizedLayerIndex` |
| `LayoutDataOne` | object | — | `m_FloatArray` |
| `LayoutDataThree` | object | — | `m_AnotherFloatArray` |
| `LayoutDataTwo` | object | — | `m_FloatValue`, `m_IntegerValue` |
| `LeafInfoConstant` | object | — | `m_IDArray`, `m_IndexOffset` |
| `LibraryRepresentation` | object | — | `name`, `scriptClassName`, `thumbnail`, `thumbnailClassID`, `flags`, `guid`, `localIdentifier`, `object`, `path` |
| `LifetimeByEmitterSpeedModule` | object | — | `enabled`, `m_Curve`, `m_Range` |
| `LightBakingOutput` | object | — | `probeOcclusionLightIndex`, `isBaked`, `lightmapBakeMode`, `lightmappingMask`, `occlusionMaskChannel`, `shadowMaskChannel` |
| `LightProbeData` | object | — | `m_NonTetrahedralizedProbeSetIndexMap`, `m_Positions`, `m_ProbeSets`, `m_Tetrahedralization` |
| `LightProbeOcclusion` | object | — | `m_Occlusion`, `m_BakedLightIndex`, `m_OcclusionMaskChannel`, `m_ProbeOcclusionLightIndex`, `m_ShadowMaskChannel` |
| `LightmapBakeMode` | object | — | `lightmapBakeType`, `mixedLightingMode` |
| `LightmapData` | object | — | `m_DirLightmap`, `m_IndirectLightmap`, `m_Lightmap`, `m_ShadowMask`, `sh_0_`, `sh_10_`, `sh_11_`, `sh_12_`, `sh_13_`, `sh_14_`, `sh_15_`, `sh_16_`, `sh_17_`, `sh_18_`, `sh_19_`, `sh_1_`, `sh_20_`, `sh_21_`, `sh_22_`, `sh_23_`, `sh_24_`, `sh_25_`, `sh_26_`, `sh_2_`, `sh_3_`, `sh_4_`, `sh_5_`, `sh_6_`, `sh_7_`, `sh_8_`, `sh_9_` |
| `LightmapSnapshot` | NamedObject | — | `m_BakedReflectionProbeCubemaps`, `m_BakedReflectionProbes`, `m_EnlightenData`, `m_EnlightenSceneMapping`, `m_EnlightenSceneMappingRendererIDs`, `m_LightProbes`, `m_LightmappedRendererData`, `m_LightmappedRendererDataIDs`, `m_Lightmaps`, `m_Lights`, `m_Name`, `m_BakedAmbientProbeInGamma`, `m_BakedAmbientProbeInLinear`, `m_BakedAmbientProbesInGamma`, `m_BakedAmbientProbesInLinear`, `m_BakedSkyboxProbeCubemaps`, `m_SceneGUID` |
| `LightsModule` | object | — | `color`, `enabled`, `intensity`, `intensityCurve`, `light`, `maxLights`, `randomDistribution`, `range`, `rangeCurve`, `ratio` |
| `Limit` | object | — | `m_Max`, `m_Min` |
| `LineParameters` | object | — | `alignment`, `colorGradient`, `endWidth`, `generateLightingData`, `m_EndColor`, `m_StartColor`, `numCapVertices`, `numCornerVertices`, `shadowBias`, `startWidth`, `textureMode`, `textureScale`, `widthCurve`, `widthMultiplier` |
| `LodSelectionCurve` | object | — | `m_LodBias`, `m_LodSlope` |
| `Lumin` | object | — | `depthFormat`, `enableGLCache`, `frameTiming`, `glCacheMaxBlobSize`, `glCacheMaxFileSize` |
| `MaterialImportOutput` | object | — | `baked`, `currentSettings` |
| `MaterialInstanceSettings` | object | — | `buildTargetSettings`, `inputs`, `materialInformation`, `materialProperties`, `name`, `prototypeName`, `textureParameters`, `lightmapFlags`, `renderQueue`, `shader`, `shaderKeywords`, `shaderName`, `textureAssignments` |
| `MatrixParameter` | object | — | `m_ArraySize`, `m_Index`, `m_NameIndex`, `m_RowCount`, `m_Type` |
| `MdFour` | object | — | `md4_hash` |
| `MeshBlendShape` | object | — | `firstVertex`, `hasNormals`, `hasTangents`, `vertexCount`, `aabbMaxDelta`, `aabbMinDelta`, `name` |
| `MeshBlendShapeChannel` | object | — | `frameCount`, `frameIndex`, `name`, `nameHash` |
| `MeshBlendShapeVertex` | object | — | `index`, `normal`, `tangent`, `vertex` |
| `MeshLodInfo` | object | — | `m_LodSelectionCurve`, `m_NumLevels`, `m_SubMeshes` |
| `MeshLodRange` | object | — | `m_IndexCount`, `m_IndexStart` |
| `MeshLodSubMesh` | object | — | `m_Levels` |
| `MinMaxAABB` | object | — | `m_Max`, `m_Min` |
| `MinMaxCurve` | object | — | `maxCurve`, `minCurve`, `minMaxState`, `scalar`, `minScalar` |
| `MinMaxGradient` | object | — | `maxColor`, `maxGradient`, `minColor`, `minGradient`, `minMaxState` |
| `MipmapLimitSettings` | object | — | `limitBias`, `limitBiasMode` |
| `Module` | object | — | `dependencies`, `name`, `strippable`, `controlledByBuiltinPackage` |
| `MonoAssemblyImporter` | AssetImporter | — | `m_ExecutionOrder`, `m_IconMap`, `m_Name`, `m_FileIDToRecycleName`, `m_NewHashIdentity`, `m_OldHashIdentity`, `m_UserData` |
| `MonoObject` | ABC | 100003 | — |
| `MotionNeighborList` | object | — | `m_NeighborArray` |
| `MultiModeParameter` | object | — | `mode`, `speed`, `spread`, `value` |
| `NameToObjectMap` | object | — | `m_ObjectToName` |
| `NativeType` | object | — | `a`, `b`, `embedded` |
| `NavMesh` | NamedObject | — | `m_Heightmaps`, `m_MeshData`, `m_Name` |
| `NavMeshAreaData` | object | — | `cost`, `name` |
| `NavMeshAreas` | GlobalGameManager | — | `areas` |
| `NavMeshBuildDebugSettings` | object | — | `m_Flags` |
| `NavMeshBuildSettings` | object | — | `agentClimb`, `agentHeight`, `agentRadius`, `agentSlope`, `agentTypeID`, `cellSize`, `ledgeDropHeight`, `manualCellSize`, `manualTileSize`, `maxJumpAcrossDistance`, `minRegionArea`, `tileSize`, `accuratePlacement`, `buildHeightMesh`, `debug`, `keepTiles`, `maxJobWorkers`, `preserveTilesOutsideBounds` |
| `NavMeshLayerData` | object | — | `cost`, `editType`, `name` |
| `NavMeshLayers` | GlobalGameManager | — | `Built_in_Layer_0`, `Built_in_Layer_1`, `Built_in_Layer_2`, `User_Layer_0`, `User_Layer_1`, `User_Layer_10`, `User_Layer_11`, `User_Layer_12`, `User_Layer_13`, `User_Layer_14`, `User_Layer_15`, `User_Layer_16`, `User_Layer_17`, `User_Layer_18`, `User_Layer_19`, `User_Layer_2`, `User_Layer_20`, `User_Layer_21`, `User_Layer_22`, `User_Layer_23`, `User_Layer_24`, `User_Layer_25`, `User_Layer_26`, `User_Layer_27`, `User_Layer_28`, `User_Layer_3`, `User_Layer_4`, `User_Layer_5`, `User_Layer_6`, `User_Layer_7`, `User_Layer_8`, `User_Layer_9` |
| `NavMeshParams` | object | — | `cellSize`, `tileSize`, `walkableClimb`, `walkableHeight`, `walkableRadius` |
| `NavMeshTileData` | object | — | `m_MeshData`, `m_Hash` |
| `NetworkViewID` | object | — | `m_ID`, `m_Type` |
| `Node` | object | — | `m_AxesId`, `m_ParentId` |
| `NoiseModule` | object | — | `damping`, `enabled`, `frequency`, `octaveMultiplier`, `octaveScale`, `octaves`, `quality`, `remap`, `remapEnabled`, `remapY`, `remapZ`, `scrollSpeed`, `separateAxes`, `strength`, `strengthY`, `strengthZ`, `positionAmount`, `rotationAmount`, `sizeAmount` |
| `NonAlignedStruct` | object | — | `m_Bool` |
| `ObjectRolePair` | object | — | `m_Object`, `m_RolesMask` |
| `OcclusionScene` | object | — | `indexPortals`, `indexRenderers`, `scene`, `sizePortals`, `sizeRenderers` |
| `Oculus` | object | — | `dashSupport`, `sharedDepthBuffer`, `lowOverheadMode`, `protectedContext`, `v2Signing` |
| `OffsetPtr` | object | — | `data` |
| `Output` | object | — | `hasEmptyFontData`, `importedType`, `previewData` |
| `PPtrCurve` | object | — | `attribute`, `classID`, `curve`, `path`, `script`, `flags` |
| `PPtrKeyframe` | object | — | `time`, `value` |
| `PackedBitVector` | object | — | `m_Data`, `m_NumItems`, `m_BitSize`, `m_Range`, `m_Start` |
| `PackingSettings` | object | — | `allowAlphaSplitting`, `blockOffset`, `enableRotation`, `enableTightPacking`, `padding`, `enableAlphaDilation` |
| `Parameter` | object | — | `m_GUID`, `m_ParameterName` |
| `ParserBindChannels` | object | — | `m_Channels`, `m_SourceMap` |
| `ParticleSystemEmissionBurst` | object | — | `cycleCount`, `repeatInterval`, `time`, `countCurve`, `maxCount`, `minCount`, `probability` |
| `ParticleSystemForceFieldParameters` | object | — | `m_DirectionCurveX`, `m_DirectionCurveY`, `m_DirectionCurveZ`, `m_DragCurve`, `m_EndRange`, `m_GravityCurve`, `m_GravityFocus`, `m_Length`, `m_MultiplyDragByParticleSize`, `m_MultiplyDragByParticleVelocity`, `m_RotationAttractionCurve`, `m_RotationRandomness`, `m_RotationSpeedCurve`, `m_Shape`, `m_StartRange`, `m_VectorField`, `m_VectorFieldAttractionCurve`, `m_VectorFieldSpeedCurve` |
| `PerLODSettings` | object | — | `castShadows`, `enableBump`, `enableHue`, `height`, `receiveShadows`, `reflectionProbeUsage`, `useLightProbes`, `windQuality`, `enableSettingOverride`, `enableSubsurface` |
| `PerformanceReportingSettings` | object | — | `m_Enabled` |
| `PhysicMaterial` | NamedObject | 134 | `bounceCombine`, `bounciness`, `dynamicFriction`, `frictionCombine`, `m_Name`, `staticFriction`, `dynamicFriction2`, `frictionDirection2`, `staticFriction2` |
| `PhysicsJobOptions2D` | object | — | `m_ClearBodyForcesPerJob`, `m_ClearFlagsPerJob`, `m_CollideContactsPerJob`, `m_FindNearestContactsPerJob`, `m_InterpolationPosesPerJob`, `m_IslandSolverBodiesPerJob`, `m_IslandSolverBodyCostScale`, `m_IslandSolverContactCostScale`, `m_IslandSolverContactsPerJob`, `m_IslandSolverCostThreshold`, `m_IslandSolverJointCostScale`, `m_NewContactsPerJob`, `m_SyncContinuousFixturesPerJob`, `m_SyncDiscreteFixturesPerJob`, `m_UpdateTriggerContactsPerJob`, `m_UseConsistencySorting`, `m_UseMultithreading`, `useConsistencySorting`, `useMultithreading` |
| `PhysicsShape` | object | — | `m_AdjacentEnd`, `m_AdjacentStart`, `m_Radius`, `m_ShapeType`, `m_UseAdjacentEnd`, `m_UseAdjacentStart`, `m_VertexCount`, `m_VertexStartIndex` |
| `PhysicsShapeGroup2D` | object | — | `m_Shapes`, `m_Vertices` |
| `PlatformSettings` | object | — | `m_AllowsAlphaSplitting`, `m_BuildTarget`, `m_CompressionQuality`, `m_CrunchedCompression`, `m_MaxTextureSize`, `m_Overridden`, `m_TextureCompression`, `m_TextureFormat`, `m_ResizeAlgorithm` |
| `PlatformSettingsData` | object | — | `settings`, `enabled` |
| `PlatformShaderDefines` | object | — | `defines_Tier1`, `defines_Tier2`, `defines_Tier3`, `shaderPlatform` |
| `PlatformShaderSettings` | object | — | `useCascadedShadowMaps`, `useScreenSpaceShadows` |
| `PluginImportOutput` | object | — | `dllType`, `pluginType`, `scriptingRuntimeVersion` |
| `Polygon2D` | ABC | 100010 | `m_Paths` |
| `PrefabModification` | object | — | `m_Modifications`, `m_RemovedComponents`, `m_TransformParent`, `m_AddedComponents`, `m_AddedGameObjects`, `m_RemovedGameObjects` |
| `PresetType` | object | — | `m_ManagedTypeFallback`, `m_ManagedTypePPtr`, `m_NativeTypeID` |
| `PreviewData` | object | — | `m_CompSize`, `m_OrigSize`, `m_PreviewData` |
| `ProbeSetIndex` | object | — | `m_Hash`, `m_Offset`, `m_Size` |
| `ProbeSetTetrahedralization` | object | — | `m_HullRays`, `m_Tetrahedra` |
| `ProceduralMaterialInformation` | object | — | `m_Offset`, `m_Scale`, `m_AnimationUpdateRate`, `m_GenerateAllOutputs`, `m_GenerateMipmaps`, `m_GeneratedAtLoading` |
| `ProceduralTextureAssignment` | object | — | `baseUID`, `material`, `shaderProp` |
| `PropertyModification` | object | — | `objectReference`, `propertyPath`, `target`, `value` |
| `PropertyModificationsTargetTestNativeObject` | object | — | `m_FloatValue`, `m_IntegerValue` |
| `QualitySetting` | object | — | `anisotropicTextures`, `antiAliasing`, `pixelLightCount`, `shadowCascades`, `shadowDistance`, `shadowProjection`, `shadowResolution`, `shadows`, `softParticles`, `softVegetation`, `vSyncCount`, `adaptiveVsync`, `adaptiveVsyncExtraA`, `adaptiveVsyncExtraB`, `asyncUploadBufferSize`, `asyncUploadPersistentBuffer`, `asyncUploadTimeSlice`, `billboardsFaceCameraPosition`, `blendWeights`, `customRenderPipeline`, `enableLODCrossFade`, `globalTextureMipmapLimit`, `lodBias`, `maximumLODLevel`, `meshLodThreshold`, `name`, `particleRaycastBudget`, `realtimeGICPUUsage`, `realtimeReflectionProbes`, `resolutionScalingFixedDPIFactor`, `shadowCascade2Split`, `shadowCascade4Split`, `shadowNearPlaneOffset`, `shadowmaskMode`, `skinWeights`, `streamingMipmapsActive`, `streamingMipmapsAddAllCameras`, `streamingMipmapsMaxFileIORequests`, `streamingMipmapsMaxLevelReduction`, `streamingMipmapsMemoryBudget`, `streamingMipmapsRenderersPerFrame`, `terrainBasemapDistance`, `terrainBillboardStart`, `terrainDetailDensityScale`, `terrainDetailDistance`, `terrainFadeLength`, `terrainMaxTrees`, `terrainPixelError`, `terrainQualityOverrides`, `terrainTreeDistance`, `textureMipmapLimitSettings`, `textureQuality`, `useLegacyDetailDistribution` |
| `QuaternionCurve` | object | — | `curve`, `path` |
| `RationalTime` | object | — | `m_Count`, `m_Rate` |
| `RayTracingShaderBuiltinSampler` | object | — | `bindPoint`, `sampler` |
| `RayTracingShaderConstantBuffer` | object | — | `byteSize`, `name`, `params`, `hash` |
| `RayTracingShaderFunctionDesc` | object | — | `attributeSizeInBytes`, `identifier`, `payloadSizeInBytes` |
| `RayTracingShaderID` | object | — | `name`, `type` |
| `RayTracingShaderParam` | object | — | `arraySize`, `colCount`, `name`, `offset`, `rowCount`, `dataSize`, `dataType`, `propertySheetType`, `type` |
| `RayTracingShaderPlatformVariant` | object | — | `dynamicKeywords`, `editorOnlyVariant`, `globalKeywords`, `localKeywords`, `targetRenderer`, `uniqueVariants`, `variantIndices` |
| `RayTracingShaderReflectionData` | object | — | `code`, `functions`, `globalResources`, `hasErrors`, `localResources`, `codeHash`, `precompiled`, `requirements` |
| `RayTracingShaderResource` | object | — | `bindPoint`, `name`, `rayGenMask`, `samplerBindPoint`, `texDimension`, `arraySize`, `multisampled` |
| `RayTracingShaderResources` | object | — | `builtinSamplers`, `constantBuffers`, `constantBuffersDesc`, `inputBuffers`, `outputBuffers`, `textures` |
| `RayTracingShaderVariant` | object | — | `resourceReflectionData`, `targetRenderer`, `editorOnlyVariant` |
| `Rectf` | object | — | `height`, `width`, `x`, `y` |
| `RenderManager` | GlobalGameManager | — | — |
| `RenderPassInfo` | object | — | `attachmentCount`, `attachments`, `depthAttachmentIndex`, `multiviewCount`, `sampleCount`, `shadingRateIndex`, `subPassCount`, `subPasses`, `foveationImageIndex`, `hasEyeTexture` |
| `RenderStateBlock` | object | — | `blendState`, `depthState`, `mask`, `rasterState`, `stencilRef`, `stencilState` |
| `RenderStateInfo` | object | — | `renderState` |
| `RendererData` | object | — | `lightmapIndex`, `lightmapIndexDynamic`, `lightmapST`, `lightmapSTDynamic`, `terrainChunkDynamicUVST`, `terrainDynamicUVST`, `uvMesh`, `explicitProbeSetHash` |
| `ResourceManager_Dependency` | object | — | `m_Dependencies`, `m_Object` |
| `RippleGroup` | object | — | `m_afDirectional_0`, `m_afDirectional_1`, `m_afDirectional_10`, `m_afDirectional_11`, `m_afDirectional_12`, `m_afDirectional_13`, `m_afDirectional_14`, `m_afDirectional_15`, `m_afDirectional_16`, `m_afDirectional_17`, `m_afDirectional_18`, `m_afDirectional_19`, `m_afDirectional_2`, `m_afDirectional_3`, `m_afDirectional_4`, `m_afDirectional_5`, `m_afDirectional_6`, `m_afDirectional_7`, `m_afDirectional_8`, `m_afDirectional_9`, `m_afFlexibility_0`, `m_afFlexibility_1`, `m_afFlexibility_10`, `m_afFlexibility_11`, `m_afFlexibility_12`, `m_afFlexibility_13`, `m_afFlexibility_14`, `m_afFlexibility_15`, `m_afFlexibility_16`, `m_afFlexibility_17`, `m_afFlexibility_18`, `m_afFlexibility_19`, `m_afFlexibility_2`, `m_afFlexibility_3`, `m_afFlexibility_4`, `m_afFlexibility_5`, `m_afFlexibility_6`, `m_afFlexibility_7`, `m_afFlexibility_8`, `m_afFlexibility_9`, `m_afPlanar_0`, `m_afPlanar_1`, `m_afPlanar_10`, `m_afPlanar_11`, `m_afPlanar_12`, `m_afPlanar_13`, `m_afPlanar_14`, `m_afPlanar_15`, `m_afPlanar_16`, `m_afPlanar_17`, `m_afPlanar_18`, `m_afPlanar_19`, `m_afPlanar_2`, `m_afPlanar_3`, `m_afPlanar_4`, `m_afPlanar_5`, `m_afPlanar_6`, `m_afPlanar_7`, `m_afPlanar_8`, `m_afPlanar_9`, `m_afSpeed_0`, `m_afSpeed_1`, `m_afSpeed_10`, `m_afSpeed_11`, `m_afSpeed_12`, `m_afSpeed_13`, `m_afSpeed_14`, `m_afSpeed_15`, `m_afSpeed_16`, `m_afSpeed_17`, `m_afSpeed_18`, `m_afSpeed_19`, `m_afSpeed_2`, `m_afSpeed_3`, `m_afSpeed_4`, `m_afSpeed_5`, `m_afSpeed_6`, `m_afSpeed_7`, `m_afSpeed_8`, `m_afSpeed_9`, `m_fIndependence`, `m_fShimmer` |
| `RootMotionData` | ABC | 100006 | — |
| `RotationBySpeedModule` | object | — | `curve`, `enabled`, `range`, `separateAxes`, `x`, `y` |
| `RotationModule` | object | — | `curve`, `enabled`, `separateAxes`, `x`, `y` |
| `SBranchWindLevel` | object | — | `m_afDirectionAdherence_0`, `m_afDirectionAdherence_1`, `m_afDirectionAdherence_2`, `m_afDirectionAdherence_3`, `m_afDirectionAdherence_4`, `m_afDirectionAdherence_5`, `m_afDirectionAdherence_6`, `m_afDirectionAdherence_7`, `m_afDirectionAdherence_8`, `m_afDirectionAdherence_9`, `m_afDistance_0`, `m_afDistance_1`, `m_afDistance_2`, `m_afDistance_3`, `m_afDistance_4`, `m_afDistance_5`, `m_afDistance_6`, `m_afDistance_7`, `m_afDistance_8`, `m_afDistance_9`, `m_afWhip_0`, `m_afWhip_1`, `m_afWhip_2`, `m_afWhip_3`, `m_afWhip_4`, `m_afWhip_5`, `m_afWhip_6`, `m_afWhip_7`, `m_afWhip_8`, `m_afWhip_9`, `m_fTurbulence`, `m_fTwitch`, `m_fTwitchFreqScale` |
| `SParams` | object | — | `BranchLevel1`, `BranchLevel2`, `LeafGroup1`, `LeafGroup2`, `Oscillation0_0`, `Oscillation0_1`, `Oscillation0_2`, `Oscillation0_3`, `Oscillation0_4`, `Oscillation0_5`, `Oscillation0_6`, `Oscillation0_7`, `Oscillation0_8`, `Oscillation0_9`, `Oscillation1_0`, `Oscillation1_1`, `Oscillation1_2`, `Oscillation1_3`, `Oscillation1_4`, `Oscillation1_5`, `Oscillation1_6`, `Oscillation1_7`, `Oscillation1_8`, `Oscillation1_9`, `Oscillation2_0`, `Oscillation2_1`, `Oscillation2_2`, `Oscillation2_3`, `Oscillation2_4`, `Oscillation2_5`, `Oscillation2_6`, `Oscillation2_7`, `Oscillation2_8`, `Oscillation2_9`, `Oscillation3_0`, `Oscillation3_1`, `Oscillation3_2`, `Oscillation3_3`, `Oscillation3_4`, `Oscillation3_5`, `Oscillation3_6`, `Oscillation3_7`, `Oscillation3_8`, `Oscillation3_9`, `Oscillation4_0`, `Oscillation4_1`, `Oscillation4_2`, `Oscillation4_3`, `Oscillation4_4`, `Oscillation4_5`, `Oscillation4_6`, `Oscillation4_7`, `Oscillation4_8`, `Oscillation4_9`, `Oscillation5_0`, `Oscillation5_1`, `Oscillation5_2`, `Oscillation5_3`, `Oscillation5_4`, `Oscillation5_5`, `Oscillation5_6`, `Oscillation5_7`, `Oscillation5_8`, `Oscillation5_9`, `Oscillation6_0`, `Oscillation6_1`, `Oscillation6_2`, `Oscillation6_3`, `Oscillation6_4`, `Oscillation6_5`, `Oscillation6_6`, `Oscillation6_7`, `Oscillation6_8`, `Oscillation6_9`, `Oscillation7_0`, `Oscillation7_1`, `Oscillation7_2`, `Oscillation7_3`, `Oscillation7_4`, `Oscillation7_5`, `Oscillation7_6`, `Oscillation7_7`, `Oscillation7_8`, `Oscillation7_9`, `Oscillation8_0`, `Oscillation8_1`, `Oscillation8_2`, `Oscillation8_3`, `Oscillation8_4`, `Oscillation8_5`, `Oscillation8_6`, `Oscillation8_7`, `Oscillation8_8`, `Oscillation8_9`, `Oscillation9_0`, `Oscillation9_1`, `Oscillation9_2`, `Oscillation9_3`, `Oscillation9_4`, `Oscillation9_5`, `Oscillation9_6`, `Oscillation9_7`, `Oscillation9_8`, `Oscillation9_9`, `m_afFrondRippleDistance_0`, `m_afFrondRippleDistance_1`, `m_afFrondRippleDistance_2`, `m_afFrondRippleDistance_3`, `m_afFrondRippleDistance_4`, `m_afFrondRippleDistance_5`, `m_afFrondRippleDistance_6`, `m_afFrondRippleDistance_7`, `m_afFrondRippleDistance_8`, `m_afFrondRippleDistance_9`, `m_afGlobalDirectionAdherence_0`, `m_afGlobalDirectionAdherence_1`, `m_afGlobalDirectionAdherence_2`, `m_afGlobalDirectionAdherence_3`, `m_afGlobalDirectionAdherence_4`, `m_afGlobalDirectionAdherence_5`, `m_afGlobalDirectionAdherence_6`, `m_afGlobalDirectionAdherence_7`, `m_afGlobalDirectionAdherence_8`, `m_afGlobalDirectionAdherence_9`, `m_afGlobalDistance_0`, `m_afGlobalDistance_1`, `m_afGlobalDistance_2`, `m_afGlobalDistance_3`, `m_afGlobalDistance_4`, `m_afGlobalDistance_5`, `m_afGlobalDistance_6`, `m_afGlobalDistance_7`, `m_afGlobalDistance_8`, `m_afGlobalDistance_9`, `m_fAnchorDistanceScale`, `m_fAnchorOffset`, `m_fDirectionResponse`, `m_fFrondRippleLightingScalar`, `m_fFrondRippleTile`, `m_fGlobalHeight`, `m_fGlobalHeightExponent`, `m_fGustDurationMax`, `m_fGustDurationMin`, `m_fGustFallScalar`, `m_fGustFrequency`, `m_fGustRiseScalar`, `m_fGustStrengthMax`, `m_fGustStrengthMin`, `m_fRollingBranchFieldMin`, `m_fRollingBranchLightingAdjust`, `m_fRollingBranchVerticalOffset`, `m_fRollingLeafRippleMin`, `m_fRollingLeafTumbleMin`, `m_fRollingNoisePeriod`, `m_fRollingNoiseSize`, `m_fRollingNoiseSpeed`, `m_fRollingNoiseTurbulence`, `m_fRollingNoiseTwist`, `m_fStrengthResponse` |
| `SWindGroup` | object | — | `m_afRippleDistance_0`, `m_afRippleDistance_1`, `m_afRippleDistance_2`, `m_afRippleDistance_3`, `m_afRippleDistance_4`, `m_afRippleDistance_5`, `m_afRippleDistance_6`, `m_afRippleDistance_7`, `m_afRippleDistance_8`, `m_afRippleDistance_9`, `m_afTumbleDirectionAdherence_0`, `m_afTumbleDirectionAdherence_1`, `m_afTumbleDirectionAdherence_2`, `m_afTumbleDirectionAdherence_3`, `m_afTumbleDirectionAdherence_4`, `m_afTumbleDirectionAdherence_5`, `m_afTumbleDirectionAdherence_6`, `m_afTumbleDirectionAdherence_7`, `m_afTumbleDirectionAdherence_8`, `m_afTumbleDirectionAdherence_9`, `m_afTumbleFlip_0`, `m_afTumbleFlip_1`, `m_afTumbleFlip_2`, `m_afTumbleFlip_3`, `m_afTumbleFlip_4`, `m_afTumbleFlip_5`, `m_afTumbleFlip_6`, `m_afTumbleFlip_7`, `m_afTumbleFlip_8`, `m_afTumbleFlip_9`, `m_afTumbleTwist_0`, `m_afTumbleTwist_1`, `m_afTumbleTwist_2`, `m_afTumbleTwist_3`, `m_afTumbleTwist_4`, `m_afTumbleTwist_5`, `m_afTumbleTwist_6`, `m_afTumbleTwist_7`, `m_afTumbleTwist_8`, `m_afTumbleTwist_9`, `m_afTwitchThrow_0`, `m_afTwitchThrow_1`, `m_afTwitchThrow_2`, `m_afTwitchThrow_3`, `m_afTwitchThrow_4`, `m_afTwitchThrow_5`, `m_afTwitchThrow_6`, `m_afTwitchThrow_7`, `m_afTwitchThrow_8`, `m_afTwitchThrow_9`, `m_fLeewardScalar`, `m_fRollMaxScale`, `m_fRollMinScale`, `m_fRollSeparation`, `m_fRollSpeed`, `m_fTwitchSharpness` |
| `SampleSettings` | object | — | `compressionFormat`, `conversionMode`, `loadType`, `quality`, `sampleRateOverride`, `sampleRateSetting`, `preloadAudioData` |
| `SamplerParameter` | object | — | `bindPoint`, `sampler` |
| `Scene` | LevelGameManager | — | `enabled`, `guid`, `m_PVSData`, `m_PVSObjectsArray`, `m_PVSPortalsArray`, `m_QueryMode`, `path` |
| `SceneDataContainer` | object | — | `m_SceneData` |
| `SceneIdentifier` | object | — | `guid`, `handle` |
| `SceneObjectIdentifier` | object | — | `targetObject`, `targetPrefab` |
| `SceneSettings` | LevelGameManager | — | `m_PVSData`, `m_PVSObjectsArray`, `m_PVSPortalsArray`, `m_QueryMode` |
| `SceneVisibilityData` | object | — | `m_SceneGUID` |
| `ScriptMapper` | GlobalGameManager | 94 | `m_Shaders`, `m_PreloadShaders` |
| `SecondarySpriteTexture` | object | — | `name`, `texture` |
| `SecondaryTextureSettings` | object | — | `platformSettings`, `sRGB` |
| `SelectorStateConstant` | object | — | `m_FullPathID`, `m_IsEntry`, `m_TransitionConstantArray` |
| `SelectorTransitionConstant` | object | — | `m_ConditionConstantArray`, `m_Destination` |
| `SerializedCustomEditorForRenderPipeline` | object | — | `customEditorName`, `renderPipelineType` |
| `SerializedPass` | object | — | `m_HasInstancingVariant`, `m_Name`, `m_NameIndices`, `m_ProgramMask`, `m_State`, `m_Tags`, `m_TextureName`, `m_Type`, `m_UseName`, `progDomain`, `progFragment`, `progGeometry`, `progHull`, `progVertex`, `m_EditorDataHash`, `m_GlobalKeywordMask`, `m_HasProceduralInstancingVariant`, `m_LocalKeywordMask`, `m_Platforms`, `m_SerializedKeywordStateMask`, `progRayTracing` |
| `SerializedPlayerSubProgram` | object | — | `m_BlobIndex`, `m_GpuProgramType`, `m_KeywordIndices`, `m_ShaderRequirements` |
| `SerializedProgram` | object | — | `m_SubPrograms`, `m_CommonParameters`, `m_ParameterBlobIndices`, `m_PlayerSubPrograms`, `m_SerializedKeywordStateMask` |
| `SerializedProgramParameters` | object | — | `m_BufferParams`, `m_ConstantBufferBindings`, `m_ConstantBuffers`, `m_MatrixParams`, `m_Samplers`, `m_TextureParams`, `m_UAVParams`, `m_VectorParams` |
| `SerializedProperties` | object | — | `m_Props` |
| `SerializedProperty` | object | — | `m_Attributes`, `m_DefTexture`, `m_DefValue_0_`, `m_DefValue_1_`, `m_DefValue_2_`, `m_DefValue_3_`, `m_Description`, `m_Flags`, `m_Name`, `m_Type` |
| `SerializedShader` | object | — | `m_CustomEditorName`, `m_Dependencies`, `m_DisableNoSubshadersMessage`, `m_FallbackName`, `m_Name`, `m_PropInfo`, `m_SubShaders`, `m_CustomEditorForRenderPipelines`, `m_KeywordFlags`, `m_KeywordNames` |
| `SerializedShaderDependency` | object | — | `from_`, `to` |
| `SerializedShaderFloatValue` | object | — | `name`, `val` |
| `SerializedShaderRTBlendState` | object | — | `blendOp`, `blendOpAlpha`, `colMask`, `destBlend`, `destBlendAlpha`, `srcBlend`, `srcBlendAlpha` |
| `SerializedShaderState` | object | — | `alphaToMask`, `culling`, `fogColor`, `fogDensity`, `fogEnd`, `fogMode`, `fogStart`, `gpuProgramID`, `lighting`, `m_LOD`, `m_Name`, `m_Tags`, `offsetFactor`, `offsetUnits`, `rtBlend0`, `rtBlend1`, `rtBlend2`, `rtBlend3`, `rtBlend4`, `rtBlend5`, `rtBlend6`, `rtBlend7`, `rtSeparateBlend`, `stencilOp`, `stencilOpBack`, `stencilOpFront`, `stencilReadMask`, `stencilRef`, `stencilWriteMask`, `zTest`, `zWrite`, `conservative`, `zClip` |
| `SerializedShaderVectorValue` | object | — | `name`, `w`, `x`, `y`, `z` |
| `SerializedStencilOp` | object | — | `comp`, `fail`, `pass_`, `zFail` |
| `SerializedSubProgram` | object | — | `m_BlobIndex`, `m_Channels`, `m_GpuProgramType`, `m_ShaderHardwareTier`, `m_BufferParams`, `m_ConstantBufferBindings`, `m_ConstantBuffers`, `m_GlobalKeywordIndices`, `m_KeywordIndices`, `m_LocalKeywordIndices`, `m_MatrixParams`, `m_Parameters`, `m_Samplers`, `m_ShaderRequirements`, `m_TextureParams`, `m_UAVParams`, `m_VectorParams` |
| `SerializedSubShader` | object | — | `m_LOD`, `m_Passes`, `m_Tags` |
| `SerializedTagMap` | object | — | `tags` |
| `SerializedTextureProperty` | object | — | `m_DefaultName`, `m_TexDim` |
| `ShaderBindChannel` | object | — | `source`, `target` |
| `ShaderInfo` | object | — | `variants` |
| `ShadowSettings` | object | — | `m_Bias`, `m_Resolution`, `m_Strength`, `m_Type`, `m_CullingMatrixOverride`, `m_CustomResolution`, `m_NearPlane`, `m_NormalBias`, `m_Softness`, `m_SoftnessFade`, `m_UseCullingMatrixOverride` |
| `ShapeModule` | object | — | `angle`, `enabled`, `m_Mesh`, `placementMode`, `radius`, `type`, `alignToDirection`, `arc`, `boxThickness`, `boxX`, `boxY`, `boxZ`, `donutRadius`, `length`, `m_MeshMaterialIndex`, `m_MeshNormalOffset`, `m_MeshRenderer`, `m_MeshScale`, `m_MeshSpawn`, `m_Position`, `m_Rotation`, `m_Scale`, `m_SkinnedMeshRenderer`, `m_Sprite`, `m_SpriteRenderer`, `m_Texture`, `m_TextureAlphaAffectsParticles`, `m_TextureBilinearFiltering`, `m_TextureClipChannel`, `m_TextureClipThreshold`, `m_TextureColorAffectsParticles`, `m_TextureUVChannel`, `m_UseMeshColors`, `m_UseMeshMaterialIndex`, `radiusThickness`, `randomDirection`, `randomDirectionAmount`, `randomPositionAmount`, `sphericalDirectionAmount` |
| `SizeBySpeedModule` | object | — | `curve`, `enabled`, `range`, `separateAxes`, `y`, `z` |
| `SizeModule` | object | — | `curve`, `enabled`, `separateAxes`, `y`, `z` |
| `Skeleton` | object | — | `m_AxesArray`, `m_ID`, `m_Node` |
| `SkeletonBone` | object | — | `m_Name`, `m_Position`, `m_Rotation`, `m_Scale`, `m_ParentName`, `m_TransformModified` |
| `SkeletonBoneLimit` | object | — | `m_Length`, `m_Max`, `m_Min`, `m_Modified`, `m_Value`, `m_PostQ`, `m_PreQ` |
| `SkeletonMask` | object | — | `m_Data` |
| `SkeletonMaskElement` | object | — | `m_Weight`, `m_Index`, `m_PathHash` |
| `SkeletonPose` | object | — | `m_X` |
| `SketchUpImportCamera` | object | — | `aspectRatio`, `fov`, `isPerspective`, `lookAt`, `orthoSize`, `position`, `up`, `farPlane`, `nearPlane` |
| `SketchUpImportData` | object | — | `defaultCamera`, `scenes` |
| `SketchUpImportScene` | object | — | `camera`, `name` |
| `SnapshotConstant` | object | — | `nameHash`, `transitionIndices`, `transitionTypes`, `values` |
| `SoftJointLimit` | object | — | `bounciness`, `limit`, `contactDistance`, `damper`, `spring` |
| `SoftJointLimitSpring` | object | — | `damper`, `spring` |
| `SortingLayerEntry` | object | — | `name`, `uniqueID`, `userID` |
| `SourceAssetIdentifier` | object | — | `assembly`, `name`, `type` |
| `SourceTextureInformation` | object | — | `doesTextureContainAlpha`, `height`, `width`, `doesTextureContainColor`, `sourceWasHDR` |
| `SpeedTreeWind` | object | — | `BRANCH_DIRECTIONAL_1`, `BRANCH_DIRECTIONAL_2`, `BRANCH_DIRECTIONAL_FROND_1`, `BRANCH_DIRECTIONAL_FROND_2`, `BRANCH_OSC_COMPLEX_1`, `BRANCH_OSC_COMPLEX_2`, `BRANCH_SIMPLE_1`, `BRANCH_SIMPLE_2`, `BRANCH_TURBULENCE_1`, `BRANCH_TURBULENCE_2`, `BRANCH_WHIP_1`, `BRANCH_WHIP_2`, `BranchWindAnchor0`, `BranchWindAnchor1`, `BranchWindAnchor2`, `FROND_RIPPLE_ADJUST_LIGHTING`, `FROND_RIPPLE_ONE_SIDED`, `FROND_RIPPLE_TWO_SIDED`, `GLOBAL_PRESERVE_SHAPE`, `GLOBAL_WIND`, `LEAF_OCCLUSION_1`, `LEAF_OCCLUSION_2`, `LEAF_RIPPLE_COMPUTED_1`, `LEAF_RIPPLE_COMPUTED_2`, `LEAF_RIPPLE_VERTEX_NORMAL_1`, `LEAF_RIPPLE_VERTEX_NORMAL_2`, `LEAF_TUMBLE_1`, `LEAF_TUMBLE_2`, `LEAF_TWITCH_1`, `LEAF_TWITCH_2`, `ROLLING`, `m_fMaxBranchLevel1Length`, `m_sParams` |
| `SpeedTreeWindConfig8` | object | — | `BRANCH_DIRECTIONAL_1`, `BRANCH_DIRECTIONAL_2`, `BRANCH_DIRECTIONAL_FROND_1`, `BRANCH_DIRECTIONAL_FROND_2`, `BRANCH_OSC_COMPLEX_1`, `BRANCH_OSC_COMPLEX_2`, `BRANCH_SIMPLE_1`, `BRANCH_SIMPLE_2`, `BRANCH_TURBULENCE_1`, `BRANCH_TURBULENCE_2`, `BRANCH_WHIP_1`, `BRANCH_WHIP_2`, `BranchLevel1`, `BranchLevel2`, `BranchWindAnchor0`, `BranchWindAnchor1`, `BranchWindAnchor2`, `FROND_RIPPLE_ADJUST_LIGHTING`, `FROND_RIPPLE_ONE_SIDED`, `FROND_RIPPLE_TWO_SIDED`, `GLOBAL_PRESERVE_SHAPE`, `GLOBAL_WIND`, `LEAF_OCCLUSION_1`, `LEAF_OCCLUSION_2`, `LEAF_RIPPLE_COMPUTED_1`, `LEAF_RIPPLE_COMPUTED_2`, `LEAF_RIPPLE_VERTEX_NORMAL_1`, `LEAF_RIPPLE_VERTEX_NORMAL_2`, `LEAF_TUMBLE_1`, `LEAF_TUMBLE_2`, `LEAF_TWITCH_1`, `LEAF_TWITCH_2`, `LeafGroup1`, `LeafGroup2`, `Oscillation0_0`, `Oscillation0_1`, `Oscillation0_2`, `Oscillation0_3`, `Oscillation0_4`, `Oscillation0_5`, `Oscillation0_6`, `Oscillation0_7`, `Oscillation0_8`, `Oscillation0_9`, `Oscillation1_0`, `Oscillation1_1`, `Oscillation1_2`, `Oscillation1_3`, `Oscillation1_4`, `Oscillation1_5`, `Oscillation1_6`, `Oscillation1_7`, `Oscillation1_8`, `Oscillation1_9`, `Oscillation2_0`, `Oscillation2_1`, `Oscillation2_2`, `Oscillation2_3`, `Oscillation2_4`, `Oscillation2_5`, `Oscillation2_6`, `Oscillation2_7`, `Oscillation2_8`, `Oscillation2_9`, `Oscillation3_0`, `Oscillation3_1`, `Oscillation3_2`, `Oscillation3_3`, `Oscillation3_4`, `Oscillation3_5`, `Oscillation3_6`, `Oscillation3_7`, `Oscillation3_8`, `Oscillation3_9`, `Oscillation4_0`, `Oscillation4_1`, `Oscillation4_2`, `Oscillation4_3`, `Oscillation4_4`, `Oscillation4_5`, `Oscillation4_6`, `Oscillation4_7`, `Oscillation4_8`, `Oscillation4_9`, `Oscillation5_0`, `Oscillation5_1`, `Oscillation5_2`, `Oscillation5_3`, `Oscillation5_4`, `Oscillation5_5`, `Oscillation5_6`, `Oscillation5_7`, `Oscillation5_8`, `Oscillation5_9`, `Oscillation6_0`, `Oscillation6_1`, `Oscillation6_2`, `Oscillation6_3`, `Oscillation6_4`, `Oscillation6_5`, `Oscillation6_6`, `Oscillation6_7`, `Oscillation6_8`, `Oscillation6_9`, `Oscillation7_0`, `Oscillation7_1`, `Oscillation7_2`, `Oscillation7_3`, `Oscillation7_4`, `Oscillation7_5`, `Oscillation7_6`, `Oscillation7_7`, `Oscillation7_8`, `Oscillation7_9`, `Oscillation8_0`, `Oscillation8_1`, `Oscillation8_2`, `Oscillation8_3`, `Oscillation8_4`, `Oscillation8_5`, `Oscillation8_6`, `Oscillation8_7`, `Oscillation8_8`, `Oscillation8_9`, `Oscillation9_0`, `Oscillation9_1`, `Oscillation9_2`, `Oscillation9_3`, `Oscillation9_4`, `Oscillation9_5`, `Oscillation9_6`, `Oscillation9_7`, `Oscillation9_8`, `Oscillation9_9`, `ROLLING`, `m_afFrondRippleDistance_0`, `m_afFrondRippleDistance_1`, `m_afFrondRippleDistance_2`, `m_afFrondRippleDistance_3`, `m_afFrondRippleDistance_4`, `m_afFrondRippleDistance_5`, `m_afFrondRippleDistance_6`, `m_afFrondRippleDistance_7`, `m_afFrondRippleDistance_8`, `m_afFrondRippleDistance_9`, `m_afGlobalDirectionAdherence_0`, `m_afGlobalDirectionAdherence_1`, `m_afGlobalDirectionAdherence_2`, `m_afGlobalDirectionAdherence_3`, `m_afGlobalDirectionAdherence_4`, `m_afGlobalDirectionAdherence_5`, `m_afGlobalDirectionAdherence_6`, `m_afGlobalDirectionAdherence_7`, `m_afGlobalDirectionAdherence_8`, `m_afGlobalDirectionAdherence_9`, `m_afGlobalDistance_0`, `m_afGlobalDistance_1`, `m_afGlobalDistance_2`, `m_afGlobalDistance_3`, `m_afGlobalDistance_4`, `m_afGlobalDistance_5`, `m_afGlobalDistance_6`, `m_afGlobalDistance_7`, `m_afGlobalDistance_8`, `m_afGlobalDistance_9`, `m_fAnchorDistanceScale`, `m_fAnchorOffset`, `m_fDirectionResponse`, `m_fFrondRippleLightingScalar`, `m_fFrondRippleTile`, `m_fGlobalHeight`, `m_fGlobalHeightExponent`, `m_fGustDurationMax`, `m_fGustDurationMin`, `m_fGustFallScalar`, `m_fGustFrequency`, `m_fGustRiseScalar`, `m_fGustStrengthMax`, `m_fGustStrengthMin`, `m_fMaxBranchLevel1Length`, `m_fRollingBranchFieldMin`, `m_fRollingBranchLightingAdjust`, `m_fRollingBranchVerticalOffset`, `m_fRollingLeafRippleMin`, `m_fRollingLeafTumbleMin`, `m_fRollingNoisePeriod`, `m_fRollingNoiseSize`, `m_fRollingNoiseSpeed`, `m_fRollingNoiseTurbulence`, `m_fRollingNoiseTwist`, `m_fStrengthResponse` |
| `SpeedTreeWindConfig9` | object | — | `m_bDoBranch1`, `m_bDoBranch2`, `m_bDoRipple`, `m_bDoShared`, `m_bDoShimmer`, `m_bLodFade`, `m_fBranch1StretchLimit`, `m_fBranch2StretchLimit`, `m_fDirectionResponse`, `m_fGustDurationMax`, `m_fGustDurationMin`, `m_fGustFallScalar`, `m_fGustFrequency`, `m_fGustRiseScalar`, `m_fGustStrengthMax`, `m_fGustStrengthMin`, `m_fSharedHeightStart`, `m_fStrengthResponse`, `m_fWindIndependence`, `m_sBranch1`, `m_sBranch2`, `m_sRipple`, `m_sShared`, `m_vTreeExtents`, `m_fImportScaling`, `pad` |
| `SphericalHarmonicsL2` | object | — | `sh_10_`, `sh_11_`, `sh_12_`, `sh_13_`, `sh_14_`, `sh_15_`, `sh_16_`, `sh_17_`, `sh_18_`, `sh_19_`, `sh_20_`, `sh_21_`, `sh_22_`, `sh_23_`, `sh_24_`, `sh_25_`, `sh_26_`, `sh__0_`, `sh__1_`, `sh__2_`, `sh__3_`, `sh__4_`, `sh__5_`, `sh__6_`, `sh__7_`, `sh__8_`, `sh__9_` |
| `SplashScreenLogo` | object | — | `duration`, `logo` |
| `SplatDatabase` | object | — | `m_AlphaTextures`, `m_AlphamapResolution`, `m_BaseMapResolution`, `m_ColorSpace`, `m_MaterialRequiresMetallic`, `m_MaterialRequiresSmoothness`, `m_Splats`, `m_TerrainLayers` |
| `SplatPrototype` | object | — | `texture`, `tileOffset`, `tileSize`, `normalMap`, `smoothness`, `specularMetallic` |
| `SpriteAtlasAssetData` | object | — | `packables` |
| `SpriteAtlasData` | object | — | `alphaTexture`, `downscaleMultiplier`, `settingsRaw`, `texture`, `textureRect`, `textureRectOffset`, `uvTransform`, `atlasRectOffset`, `secondaryTextures` |
| `SpriteAtlasEditorData` | object | — | `bindAsDefault`, `cachedData`, `isAtlasV2`, `packables`, `packingSettings`, `platformSettings`, `textureSettings`, `variantMultiplier`, `secondaryTextureSettings`, `storedHash`, `totalSpriteSurfaceArea` |
| `SpriteBone` | object | — | `length`, `name`, `parentId`, `position`, `rotation`, `color`, `guid` |
| `SpriteCustomDataEntry` | object | — | `m_Key`, `m_Value` |
| `SpriteCustomMetadata` | object | — | `m_Entries` |
| `SpriteData` | object | — | `sprite` |
| `SpriteMetaData` | object | — | `m_Alignment`, `m_Name`, `m_Pivot`, `m_Rect`, `m_Bones`, `m_Border`, `m_CustomData`, `m_Edges`, `m_Indices`, `m_InternalID`, `m_Outline`, `m_PhysicsShape`, `m_SpriteID`, `m_TessellationDetail`, `m_Vertices`, `m_Weights` |
| `SpriteRenderData` | object | — | `settingsRaw`, `texture`, `textureRect`, `textureRectOffset`, `alphaTexture`, `atlasRectOffset`, `downscaleMultiplier`, `indices`, `m_Bindpose`, `m_IndexBuffer`, `m_SourceSkin`, `m_SubMeshes`, `m_VertexData`, `secondaryTextures`, `uvTransform`, `vertices` |
| `SpriteSheetMetaData` | object | — | `m_Sprites`, `m_Bones`, `m_CustomData`, `m_Edges`, `m_Indices`, `m_InternalID`, `m_NameFileIdTable`, `m_Outline`, `m_PhysicsShape`, `m_SecondaryTextures`, `m_SpriteCustomMetadata`, `m_SpriteID`, `m_Vertices`, `m_Weights` |
| `SpriteTilingProperty` | object | — | `adaptiveTiling`, `adaptiveTilingThreshold`, `border`, `drawMode`, `newSize`, `oldSize`, `pivot` |
| `SpriteVertex` | object | — | `pos`, `uv` |
| `State` | NamedObject | — | `m_IKOnFeet`, `m_Motions`, `m_Name`, `m_ParentStateMachine`, `m_Position`, `m_Speed`, `m_Tag`, `m_CycleOffset`, `m_Mirror` |
| `StateConstant` | object | — | `m_BlendTreeConstantArray`, `m_BlendTreeConstantIndexArray`, `m_IKOnFeet`, `m_Loop`, `m_Speed`, `m_TagID`, `m_TransitionConstantArray`, `m_CycleOffset`, `m_CycleOffsetParamID`, `m_FullPathID`, `m_ID`, `m_LeafInfoArray`, `m_Mirror`, `m_MirrorParamID`, `m_NameID`, `m_PathID`, `m_SpeedParamID`, `m_TimeParamID`, `m_WriteDefaultValues` |
| `StateKey` | object | — | `m_LayerIndex`, `m_StateID` |
| `StateMachine` | NamedObject | — | `m_AnyStatePosition`, `m_ChildStateMachine`, `m_ChildStateMachinePosition`, `m_DefaultState`, `m_MotionSetCount`, `m_Name`, `m_OrderedTransitions`, `m_ParentStateMachinePosition`, `m_States`, `m_LocalTransitions` |
| `StateMachineBehaviourVectorDescription` | object | — | `m_StateMachineBehaviourIndices`, `m_StateMachineBehaviourRanges` |
| `StateMachineConstant` | object | — | `m_AnyStateTransitionConstantArray`, `m_DefaultState`, `m_StateConstantArray`, `m_EvaluateTransitionsOnStart`, `m_MotionSetCount`, `m_SelectorStateConstantArray`, `m_SynchronizedLayerCount` |
| `StateRange` | object | — | `m_Count`, `m_StartIndex` |
| `StaticBatchInfo` | object | — | `firstSubMesh`, `subMeshCount` |
| `StreamInfo` | object | — | `channelMask`, `offset`, `stride`, `align`, `dividerOp`, `frequency` |
| `StreamedClip` | object | — | `curveCount`, `data`, `discreteCurveCount` |
| `StreamedResource` | object | — | `m_Offset`, `m_Size`, `m_Source` |
| `StreamingInfo` | object | — | `offset`, `path`, `size` |
| `StructParameter` | object | — | `m_ArraySize`, `m_Index`, `m_MatrixMembers`, `m_NameIndex`, `m_StructSize`, `m_VectorMembers` |
| `SubCollider` | object | — | `m_Collider`, `m_ColliderPaths` |
| `SubEmitterData` | object | — | `emitter`, `properties`, `type`, `emitProbability` |
| `SubMesh` | object | — | `firstByte`, `firstVertex`, `indexCount`, `localAABB`, `vertexCount`, `baseVertex`, `isTriStrip`, `topology`, `triangleCount` |
| `SubModule` | object | — | `enabled`, `subEmitterBirth`, `subEmitterBirth1`, `subEmitterCollision`, `subEmitterCollision1`, `subEmitterDeath`, `subEmitterDeath1`, `subEmitters` |
| `SubPassDescriptor` | object | — | `colorOutputs`, `flags`, `inputs` |
| `SubstanceEnumItem` | object | — | `text`, `value` |
| `SubstanceInput` | object | — | `alteredTexturesUID`, `enumValues`, `flags`, `internalIndex`, `internalType`, `maximum`, `minimum`, `name`, `step`, `type`, `value`, `componentLabels`, `group`, `internalIdentifier`, `label`, `visibleIf` |
| `SubstanceValue` | object | — | `scalar_0_`, `scalar_1_`, `scalar_2_`, `scalar_3_`, `texture`, `stringvalue` |
| `TakeInfo` | object | — | `bakeStartTime`, `bakeStopTime`, `clip`, `defaultClipName`, `name`, `sampleRate`, `startTime`, `stopTime`, `internalID` |
| `Tetrahedron` | object | — | `indices_0_`, `indices_1_`, `indices_2_`, `indices_3_`, `matrix`, `neighbors_0_`, `neighbors_1_`, `neighbors_2_`, `neighbors_3_` |
| `TextureImportInstructions` | object | — | `colorSpace`, `compressedFormat`, `compressionQuality`, `height`, `uncompressedFormat`, `usageMode`, `width`, `androidETC2FallbackDownscale`, `androidETC2FallbackFormat`, `cubeIntermediateSize`, `cubeLayout`, `cubeMode`, `depth`, `desiredFormat`, `recommendedFormat`, `vtOnly` |
| `TextureImportOutput` | object | — | `sourceTextureInformation`, `textureImportInstructions`, `importInspectorWarnings` |
| `TextureImporterPlatformSettings` | object | — | `m_AllowsAlphaSplitting`, `m_AndroidETC2FallbackOverride`, `m_BuildTarget`, `m_CompressionQuality`, `m_CrunchedCompression`, `m_MaxTextureSize`, `m_Overridden`, `m_ResizeAlgorithm`, `m_TextureCompression`, `m_TextureFormat`, `m_ForceMaximumCompressionQuality_BC6H_BC7`, `m_IgnorePlatformSupport` |
| `TextureParameter` | object | — | `m_Dim`, `m_Index`, `m_NameIndex`, `m_SamplerIndex`, `m_MultiSampled` |
| `TextureParameters` | object | — | `height`, `mipLevels`, `textureFormat`, `width` |
| `TextureSettings` | object | — | `anisoLevel`, `compressionQuality`, `crunchedCompression`, `filterMode`, `generateMipMaps`, `maxTextureSize`, `readable`, `sRGB`, `textureCompression` |
| `TicksPerSecond` | object | — | `m_Denominator`, `m_Numerator` |
| `TierGraphicsSettings` | object | — | `renderingPath`, `useCascadedShadowMaps`, `enableLPPV`, `hdrMode`, `prefer32BitShadowMaps`, `realtimeGICPUUsage`, `useHDR` |
| `Tile` | object | — | `m_TileColorIndex`, `m_TileIndex`, `m_TileMatrixIndex`, `m_TileSpriteIndex`, `dummyAlignment`, `m_AllTileFlags`, `m_ColliderType`, `m_ObjectToInstantiate`, `m_TileFlags`, `m_TileObjectToInstantiateIndex` |
| `TileAnimationData` | object | — | `m_AnimatedSprites`, `m_AnimationSpeed`, `m_AnimationTimeOffset`, `m_Flags`, `m_IsLooping` |
| `TilemapRefCountedData` | object | — | `m_Data`, `m_RefCount` |
| `TrailModule` | object | — | `colorOverLifetime`, `colorOverTrail`, `dieWithParticles`, `enabled`, `inheritParticleColor`, `lifetime`, `minVertexDistance`, `ratio`, `sizeAffectsLifetime`, `sizeAffectsWidth`, `textureMode`, `widthOverTrail`, `worldSpace`, `attachRibbonsToTransform`, `generateLightingData`, `mode`, `ribbonCount`, `shadowBias`, `splitSubEmitterRibbons`, `textureScale` |
| `TransformMaskElement` | object | — | `m_Path`, `m_Weight` |
| `Transition` | NamedObject | — | `m_Atomic`, `m_Conditions`, `m_DstState`, `m_Mute`, `m_Name`, `m_Solo`, `m_SrcState`, `m_TransitionDuration`, `m_TransitionOffset`, `m_CanTransitionToSelf` |
| `TransitionConstant` | object | — | `m_ConditionConstantArray`, `m_DestinationState`, `m_ID`, `m_TransitionDuration`, `m_TransitionOffset`, `m_UserID`, `m_Atomic`, `m_CanTransitionToSelf`, `m_ExitTime`, `m_FullPathID`, `m_HasExitTime`, `m_HasFixedDuration`, `m_InterruptionSource`, `m_OrderedInterruption` |
| `TreeInstance` | object | — | `color`, `heightScale`, `index`, `lightmapColor`, `position`, `widthScale`, `rotation` |
| `TreePrototype` | object | — | `bendFactor`, `prefab`, `navMeshLod` |
| `TriggerModule` | object | — | `enabled`, `enter`, `exit`, `inside`, `outside`, `radiusScale`, `colliderQueryMode`, `collisionShape0`, `collisionShape1`, `collisionShape2`, `collisionShape3`, `collisionShape4`, `collisionShape5`, `primitives` |
| `TypeStats` | object | — | `classID`, `objectCount`, `resourceCount`, `size` |
| `UAVParameter` | object | — | `m_Index`, `m_NameIndex`, `m_OriginalIndex` |
| `UVAnimation` | object | — | `cycles`, `x_Tile`, `y_Tile` |
| `UVModule` | object | — | `animationType`, `cycles`, `enabled`, `frameOverTime`, `rowIndex`, `tilesX`, `tilesY`, `flipU`, `flipV`, `fps`, `mode`, `randomRow`, `rowMode`, `speedRange`, `sprites`, `startFrame`, `timeMode`, `uvChannelMask` |
| `UnityAdsSettings` | GlobalGameManager | — | `m_Enabled`, `m_InitializeOnStartup`, `m_TestMode`, `m_AndroidGameId`, `m_EnabledPlatforms`, `m_GameId`, `m_IosGameId` |
| `UnityAnalyticsSettings` | object | — | `m_Enabled`, `m_TestMode`, `m_InitializeOnStartup`, `m_PackageRequiringCoreStatsPresent`, `m_TestConfigUrl`, `m_TestEventUrl` |
| `UnityPropertySheet` | object | — | `m_Colors`, `m_Floats`, `m_TexEnvs`, `m_Ints` |
| `UnityPurchasingSettings` | object | — | `m_Enabled`, `m_TestMode` |
| `UnitySceneHandle` | object | — | `value` |
| `UnityTexEnv` | object | — | `m_Offset`, `m_Scale`, `m_Texture` |
| `UpdateZoneInfo` | object | — | `needSwap`, `passIndex`, `rotation`, `updateZoneCenter`, `updateZoneSize` |
| `VFXCPUBufferData` | object | — | `data` |
| `VFXCPUBufferDesc` | object | — | `capacity`, `initialData`, `layout`, `stride` |
| `VFXEditorSystemDesc` | object | — | `buffers`, `capacity`, `flags`, `layer`, `tasks`, `type`, `values`, `name` |
| `VFXEditorTaskDesc` | object | — | `buffers`, `params`, `processor`, `shaderSourceIndex`, `type`, `values`, `temporaryBuffers` |
| `VFXEntryExposed` | object | — | `m_Name`, `m_Overridden`, `m_Value` |
| `VFXEntryExpressionValue` | object | — | `m_ExpressionIndex`, `m_Value` |
| `VFXEventDesc` | object | — | `name`, `playSystems`, `stopSystems`, `initSystems` |
| `VFXExposedMapping` | object | — | `mapping`, `space` |
| `VFXExpressionContainer` | object | — | `m_Expressions`, `m_NeedsLocalToWorld`, `m_NeedsWorldToLocal`, `m_ConstantBakeCurveCount`, `m_ConstantBakeGradientCount`, `m_DynamicBakeCurveCount`, `m_DynamicBakeGradientCount`, `m_MaxCommonExpressionsIndex`, `m_NeededMainCameraBuffers`, `m_NeedsMainCamera` |
| `VFXField` | object | — | `m_Array` |
| `VFXGPUBufferDesc` | object | — | `capacity`, `layout`, `size`, `stride`, `mode`, `target`, `type` |
| `VFXInstanceSplitDesc` | object | — | `values` |
| `VFXLayoutElementDesc` | object | — | `name`, `offset`, `type` |
| `VFXLayoutOffset` | object | — | `bucket`, `element`, `structure` |
| `VFXMapping` | object | — | `index`, `nameId` |
| `VFXMappingTemporary` | object | — | `mapping`, `pastFrameIndex`, `perCameraBuffer` |
| `VFXPropertySheetSerializedBase` | object | — | `m_AnimationCurve`, `m_Bool`, `m_Float`, `m_Gradient`, `m_Int`, `m_Matrix4x4f`, `m_NamedObject`, `m_Uint`, `m_Vector2f`, `m_Vector3f`, `m_Vector4f` |
| `VFXRendererSettings` | object | — | `motionVectorGenerationMode`, `shadowCastingMode`, `lightProbeUsage`, `rayTracingMode`, `receiveShadows`, `reflectionProbeUsage`, `transparencyPriority` |
| `VFXShaderSourceDesc` | object | — | `compute`, `name`, `source` |
| `VFXSystemDesc` | object | — | `buffers`, `capacity`, `flags`, `layer`, `tasks`, `type`, `values`, `instanceSplitDescs`, `name` |
| `VFXTaskDesc` | object | — | `buffers`, `params`, `processor`, `type`, `values`, `instanceSplitIndex`, `temporaryBuffers` |
| `VFXTemplate` | object | — | `category`, `description`, `icon`, `name`, `thumbnail`, `order` |
| `VFXTemporaryGPUBufferDesc` | object | — | `desc`, `frameCount` |
| `VRSettings` | object | — | `cardboard`, `daydream`, `enable360StereoCapture`, `hololens`, `lumin`, `none`, `oculus` |
| `ValueArray` | object | — | `m_BoolValues`, `m_FloatValues`, `m_IntValues`, `m_EntityIdValues`, `m_PositionValues`, `m_QuaternionValues`, `m_ScaleValues`, `m_VectorValues` |
| `ValueArrayConstant` | object | — | `m_ValueArray` |
| `ValueConstant` | object | — | `m_ID`, `m_Index`, `m_Type`, `m_TypeID` |
| `ValueDelta` | object | — | `m_Start`, `m_Stop` |
| `VariableBoneCountWeights` | object | — | `m_Data` |
| `VariantInfo` | object | — | `graphicsStateInfoSet`, `keywordNames`, `keywords`, `passIndex`, `passType`, `shader`, `shaderAssetGUID`, `shaderAssetLocalIdentifierInFile`, `shaderName`, `subShaderIndex` |
| `Vector3Curve` | object | — | `curve`, `path` |
| `VectorParameter` | object | — | `m_ArraySize`, `m_Dim`, `m_Index`, `m_NameIndex`, `m_Type` |
| `VelocityModule` | object | — | `enabled`, `inWorldSpace`, `x`, `y`, `z`, `orbitalOffsetX`, `orbitalOffsetY`, `orbitalOffsetZ`, `orbitalX`, `orbitalY`, `orbitalZ`, `radial`, `speedModifier` |
| `VertexData` | object | — | `m_DataSize`, `m_VertexCount`, `m_Channels`, `m_CurrentChannels`, `m_Streams`, `m_Streams_0_`, `m_Streams_1_`, `m_Streams_2_`, `m_Streams_3_` |
| `VertexLayoutInfo` | object | — | `vertexChannelsInfo`, `vertexStreamCount`, `vertexStrides` |
| `VideoClipImporterOutput` | object | — | `encodedEndFrame`, `encodedHeight`, `encodedSettings`, `encodedStartFrame`, `encodedWidth`, `format`, `originalFrameCount`, `originalHeight`, `originalWidth`, `settings`, `sourceAudioChannelCount`, `sourceAudioSampleRate`, `sourceFileSize`, `sourceFrameRate`, `sourceHasAlpha`, `sourcePixelAspectRatioDenominator`, `sourcePixelAspectRatioNumerator`, `streamedResource`, `transcodeSkipped` |
| `VideoClipImporterTargetSettings` | object | — | `aspectRatio`, `bitrateMode`, `codec`, `customHeight`, `customWidth`, `enableTranscoding`, `resizeFormat`, `spatialQuality` |
| `VisualEffectInfo` | object | — | `m_Buffers`, `m_CPUBuffers`, `m_CullingFlags`, `m_Events`, `m_ExposedExpressions`, `m_Expressions`, `m_PropertySheet`, `m_RendererSettings`, `m_UpdateMode`, `m_CompilationVersion`, `m_InitialEventName`, `m_InstancingCapacity`, `m_InstancingDisabledReason`, `m_InstancingMode`, `m_PreWarmDeltaTime`, `m_PreWarmStepCount`, `m_RuntimeVersion`, `m_TemporaryBuffers` |
| `VisualEffectSettings` | object | — | `m_CullingFlags`, `m_InitialEventName`, `m_PreWarmDeltaTime`, `m_PreWarmStepCount`, `m_RendererSettings`, `m_UpdateMode`, `m_InstancingCapacity`, `m_InstancingDisabledReason`, `m_InstancingMode` |
| `VulkanGraphicsJobsDeviceFilterData` | object | — | `filter`, `preferredMode` |
| `WheelFrictionCurve` | object | — | `asymptoteSlip`, `asymptoteValue`, `extremumSlip`, `extremumValue`, `m_AsymptoteSlip`, `m_AsymptoteValue`, `m_ExtremumSlip`, `m_ExtremumValue`, `m_Stiffness`, `stiffnessFactor` |
| `bitset` | object | — | `bitCount`, `bitblocks` |
| `int2_storage` | object | — | `x`, `y` |
| `int3_storage` | object | — | `x`, `y`, `z` |
| `xform` | object | — | `q`, `s`, `t` |

## Appendix B. Fields and members of UnityPy's own classes

The attrs fields and the public members (`inventory.py` groups `fields` and `methods`) of the classes UnityPy defines outside `classes/generated.py` that no row above names one by one. The tier and level are those of the class's row in the section given; a member listed under a base class (`EndianBinaryReader`) is inherited by its subclasses, which list only what they redefine.

| class | covered in | tier | fields | members |
|---|---|---|---|---|
| `SerializedFileHeader` | 7.2 | — | `data_offset`, `endian`, `file_size`, `metadata_size`, `reserved`, `version` | — |
| `SerializedType` | 7.2 | A | `class_id`, `is_stripped_type`, `m_AssemblyName`, `m_ClassName`, `m_NameSpace`, `node`, `old_type_hash`, `script_id`, `script_type_index`, `type_dependencies` | — |
| `FileIdentifier` | 7.2 | A | `guid`, `path`, `temp_empty`, `type` | `name`, `write` |
| `LocalSerializedObjectIdentifier` | 7.2 | A | `local_identifier_in_file`, `local_serialized_file_index` | `write` |
| `ObjectReader` | 8 | A | `assets_file`, `byte_size`, `byte_start`, `class_id`, `data`, `is_destroyed`, `is_stripped`, `path_id`, `reader`, `serialized_type`, `type`, `type_id` | — |
| `TypeTreeConfig` | 9.1 | — | `as_dict`, `assetsfile`, `has_registry` | `copy` |
| `TpkClassInformation` | 10 | — | — | `getVersionedClass` |
| `TpkCommonString` | 10 | — | — | `GetCount`, `GetStrings` |
| `TpkDataType` | 10 | — | — | `ToBlob` |
| `TpkFile` | 10 | — | — | `GetDataBlob` |
| `TpkUnityClass` | 10 | — | — | `to_dict` |
| `TypeTreeGeneratorBase` | 10 | — | — | `get_nodes`, `get_nodes_as_json`, `load_dll`, `load_il2cpp` |
| `NodeClass` | 10 | — | — | `generate_str`, `sort_fields` |
| `NodeClassField` | 10 | — | — | `clean_name`, `generate_str` |
| `EndianBinaryReader` | 13 | — | — | `align_stream`, `bytes`, `endian`, `read`, `read_aligned_string`, `read_array`, `read_array_struct`, `read_boolean`, `read_boolean_array`, `read_byte`, `read_byte_array`, `read_bytes`, `read_double`, `read_double_array`, `read_float`, `read_float_array`, `read_int`, `read_int_array`, `read_long`, `read_long_array`, `read_short`, `read_short_array`, `read_string`, `read_string_array`, `read_string_to_null`, `read_the_rest`, `read_u_byte`, `read_u_byte_array`, `read_u_int`, `read_u_int_array`, `read_u_long`, `read_u_long_array`, `read_u_short`, `read_u_short_array`, `real_offset`, `seek`, `tell` |
| `EndianBinaryReader_Memoryview` | 13 | — | — | `bytes`, `dispose`, `read`, `read_aligned_string`, `read_array_struct`, `read_string_to_null` |
| `EndianBinaryReader_Streamable` | 13 | — | — | `Length`, `Position`, `bytes`, `dispose` |
| `EndianBinaryWriter` | 13 | — | — | `Length`, `align_stream`, `bytes`, `dispose`, `write`, `write_aligned_string`, `write_array`, `write_boolean`, `write_boolean_array`, `write_byte`, `write_byte_array`, `write_bytes`, `write_double`, `write_float`, `write_float_array`, `write_int`, `write_int_array`, `write_long`, `write_short`, `write_string_array`, `write_string_to_null`, `write_u_byte`, `write_u_int`, `write_u_int_array`, `write_u_long`, `write_u_short`, `write_u_short_array` |
| `PPtr` | 14.1 | A | `assetsfile`, `m_FileID`, `m_PathID` | — |
| `Vector2f` | 14.1 | A | `x`, `y` | — |
| `Vector3f` | 14.1 | A | `x`, `y`, `z` | — |
| `Vector4f` | 14.1 | A | `w`, `x`, `y`, `z` | — |
| `Quaternionf` | 14.1 | A | `w`, `x`, `y`, `z` | — |
| `Matrix3x4f` | 14.1 | A | `e00`, `e01`, `e02`, `e03`, `e10`, `e11`, `e12`, `e13`, `e20`, `e21`, `e22`, `e23` | — |
| `Matrix4x4f` | 14.1 | A | `e00`, `e01`, `e02`, `e03`, `e10`, `e11`, `e12`, `e13`, `e20`, `e21`, `e22`, `e23`, `e30`, `e31`, `e32`, `e33` | — |
| `ColorRGBA` | 14.1 | A | `a`, `b`, `g`, `r` | `rgba` |
| `ContainerHelper` | 14.3 | A | `container`, `container_dict` | — |
| `ShaderProgram` | 15.5 | A | — | `Export` |
| `ShaderSubProgram` | 15.5 | A | — | `Export` |
