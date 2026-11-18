# Build the unityrs fixtures in Unity

1. Install **Unity 2022.3 LTS** with Unity Hub.
2. In Unity Hub, create and open an empty **3D (Built-in)** project. Use the
   default desktop build target.
3. Copy the **Editor** folder at `tools/fixtures/unity/Editor` into the Unity
   project's **Assets** folder. The result should be `Assets/Editor`. Switch
   back to Unity and wait for it to finish importing the scripts.
4. In Unity's top menu, click **Tools → unityrs → Build and record all
   fixtures**. Wait for the dialog that gives the Unity version, build target,
   and output location.
5. Open `UnityrsFixturesOutput/<unity-version>/<target>/` in the project
   folder, next to `Assets`. Copy that whole version-and-target folder back,
   including `bundles/`, `replays/`, and `build.log`.

If the dialog reports replay errors, send the output folder with the log.
If bundle building fails, send `build.log` from the location in the dialog.
If Unity never shows the menu, open **Window → General → Console** and send
the first red error. Generated project assets live in `Assets/UnityrsFixtures/`.
Re-running replaces the previous output for the same Editor version and target.

## What the button builds

| Bundle or replay | Coverage |
|---|---|
| `static-objects` | Cube, Cylinder, Sphere, Capsule, Plane, Quad, and their Built-in default material. Two LOD levels, Standard and Unlit materials with an original checker texture, and a four-influence skinned mesh with two renderers and blend-shape frames at 25 and 80. |
| `static-batch-scene` | Two root objects in authored order with different transforms, combined with `StaticBatchingUtility.Combine`. |
| `legacy-explicit` | Legacy Animation, position and explicit quaternion keys, and a non-ASCII child binding. |
| `generic-quaternion`, `generic-euler` | Generic Animator clips with quaternion tangents and multi-axis Euler keys including a full turn. |
| `generic-constant`, `generic-offset-stepped`, `generic-weighted` | Constant-only values, a nonzero start with a held jump, and asymmetric weighted tangents. |
| `generic-morph`, `generic-override-morph` | Independently animated weights on two renderers sharing one mesh, four-influence bone animation, and an override controller's replacement clip. |
| `generic-null-override` | The original constant clip selected through an explicit null override. |
| `generic-object-reference` | Initial and changing mesh references and a discrete activation curve. |

URP materials are skipped because they require the URP package and a URP
project. The standard build uses procedural assets and Unity's Built-in
resources. Humanoid playback requires an imported rig and Avatar. For a
later optional run, place an original animated humanoid model you can
redistribute at `Assets/UnityrsHumanoid.fbx` before clicking the same
button. Unity must be able to map the skeleton and import an animation
take. The build log reports whether it added the optional humanoid replay.

The same menu also offers **Build fixtures only**, the manual **Record
animation replay** command, and **Test animation replay prefab loading**.
