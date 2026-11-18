# Build the unityrs fixtures on Windows

There are three runs for each Unity version, **2022.3.62f3** first and then
**6000.6.9f1**:

| Run | Project | Menu item | Folder to send |
| --- | --- | --- | --- |
| Main | new **3D (Built-in)** | **Build and record all fixtures** | `<Unity version>/StandaloneWindows64` |
| URP materials | new **Universal 3D** | **Build URP material fixtures** | `<Unity version>/URP-StandaloneWindows64` |
| Android | the same Built-in project | **Build Android fixtures** | `<Unity version>/Android-variant` |

All three menu items are under **Tools → unityrs** and come from the same
**Editor** folder. Each folder is inside `UnityrsFixturesOutput` in the
project that ran it. The main run also builds the optional coverage
fixtures (two humanoid refusal probes). When Unity cannot provide one of
them, `run.json` lists it as not covered and the dialog still says **ready**.
The integer discrete binding is always listed as not covered.

1. Open the [unityrs repository on GitHub](https://github.com/scobenes/unityrs).
   Select the branch or commit you were given in the branch menu, then
   click **Code → Download ZIP**. Extract the ZIP.
2. In Unity Hub, create and open a **new 3D (Built-in)** project using
   **Unity 2022.3.62f3**. Any Unity **2022.3 LTS** patch works. Keep the
   project path short, for example `C:\unity\unityrs-fixtures-2022`. In File
   Explorer, find `tools/fixtures/unity/Editor` in the downloaded repository
   and copy the whole **Editor** folder, including its `Urp` subfolder, into
   the project's **Assets** folder. The result should be `Assets/Editor`. If
   the project already has an `Assets/Editor` folder from an earlier
   download, delete it first so no old scripts remain. Return to Unity and wait for the
   scripts to finish importing.
3. Exit Play Mode and close any animation preview. In Unity, choose
   **Tools → unityrs → Build and record all fixtures**. Save the scene if
   Unity asks, then wait for the results dialog.
4. If the dialog says **unityrs fixtures ready** or **unityrs fixtures built
   with errors**, zip the folder named after your Unity version (e.g.
   `2022.3.62f3`) inside `UnityrsFixturesOutput`, and send the zip (USB stick
   or cloud drive). `UnityrsFixturesOutput` is in your project folder, next to **Assets**.
   Keep `bundles`, `replays`, `run.json`, and `build.log` together.
   A new build that reaches this dialog replaces the previous results in that folder.
   If the dialog says **unityrs fixtures staged**, close windows or terminals
   using the output folders and zip the staged folder shown in the dialog.
5. If the dialog says **unityrs fixture build failed**, follow the path in the
   dialog. Under `UnityrsFixturesOutput/<Unity version>`, a folder named
   `<target>-failed-<id>` holds the copied `build.log` to send. If Unity
   could not close its temporary fixture scene, zip and send the
   `<target>.b-<id>` folder with `build.log` inside. Unity builds in this
   staging folder before moving the results to `<target>`. The ID is the
   first eight characters of the run ID.
   If the menu never appears or the log is missing, open
   **Window → General → Console** and send the first red error.
6. Later, repeat steps 2–5 in a **separate new 3D (Built-in) project** using
   **Unity 6000.x**, for example **6000.6.9f1**. Keep the 2022.3 project in
   its original Unity version. The URP and Android runs below follow the same
   order: 2022.3 first, then 6000.x.

## URP material fixtures

1. In Unity Hub, create a **new Universal 3D** project (the template is
   also called **3D (URP)**) using **Unity 2022.3.62f3**. Use a separate
   project from the Built-in one, for example `C:\unity\unityrs-urp-2022`.
2. Copy the same **Editor** folder into the project's **Assets** folder, so
   the result is `Assets/Editor`, and wait for the scripts to import. As in
   the main run, replace any older `Assets/Editor` folder.
3. Choose **Tools → unityrs → Build URP material fixtures**. If the dialog
   says the project needs URP, the project was not made from the Universal
   3D template: create a new one as in step 1.
4. When the dialog says **unityrs fixtures ready**, zip the
   `UnityrsFixturesOutput/<Unity version>/URP-StandaloneWindows64` folder
   and send it. Follow steps 4–5 above for the other dialogs.
5. Later, repeat steps 1–4 in a separate new Universal 3D project using
   **Unity 6000.x**, for example **6000.6.9f1**.

## Android fixtures

1. Close Unity. In Unity Hub, open **Installs**, click the gear next to
   **2022.3.62f3**, choose **Add modules**, tick **Android Build Support**
   and click **Install**. Unity Hub ticks the OpenJDK and Android SDK & NDK
   tools under it; keep them ticked.
2. Open the **3D (Built-in)** project from step 2 above. The active
   platform can stay **Windows**: the menu item builds for Android itself.
3. Choose **Tools → unityrs → Build Android fixtures**. If the dialog says
   **Android Build Support** is missing, repeat step 1. Building for
   Android from a Windows project switches the build target for the
   duration of the build: expect Unity to recompile scripts and reimport
   the textures, which can take several minutes, and save the scene if
   Unity asks.
4. When the dialog says **unityrs fixtures ready**, zip the
   `UnityrsFixturesOutput/<Unity version>/Android-variant` folder and send
   it.
   Follow steps 4–5 above for the other dialogs.
5. Later, add the Android module to **6000.6.9f1** too and repeat steps 2–4
   in the Unity 6000.x Built-in project.

Each project has its own `UnityrsFixturesOutput` folder. In the Built-in
project, the Windows and Android results sit side by side in the
`<Unity version>` folder, so zipping that whole folder sends both.

If the maintainer needs a manual FBX fallback, use a **CC0-licensed rigged
model**. Exclude TurboSquid and Mixamo assets because their licences forbid
redistributing the rigged assets in the fixtures.
