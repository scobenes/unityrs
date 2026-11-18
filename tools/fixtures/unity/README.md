# Build the unityrs fixtures on Windows

1. Open the [public unityrs repository](https://github.com/scobenes/unityrs)
   on GitHub. Click **Code → Download ZIP**, then extract the ZIP. If you
   already use Git, you can instead run
   `git clone https://github.com/scobenes/unityrs.git`.
2. In Unity Hub, create and open a **new 3D (Built-in)** project using
   **Unity 2022.3.62f3**. Any Unity **2022.3 LTS** patch works. In File
   Explorer, find `tools/fixtures/unity/Editor` in the downloaded repository
   and copy the whole **Editor** folder into the project's **Assets** folder.
   The result should be `Assets/Editor`. Return to Unity and wait for the
   scripts to finish importing. The rigs and animations are generated
   automatically.
3. In Unity, choose **Tools → unityrs → Build and record all fixtures**.
   Wait for the results dialog. The menu uses the project's active build
   target. With the default Windows desktop target on your PC the result is
   **StandaloneWindows64**.
4. In File Explorer, open your project folder (the one containing **Assets**)
   and find `UnityrsFixturesOutput\<version>\<target>\`. For example, the
   first run on 2022.3.62f3 uses
   `UnityrsFixturesOutput\2022.3.62f3\StandaloneWindows64\`.
   Copy the entire target folder, with its version folder above it, to
   **the maintainer's fixture inbox** on the Mac. You can use a USB drive,
   cloud drive, or network share. Keep `bundles`, `replays`, and `build.log`
   together. Running the button again replaces the previous output for that
   Unity version and target.
5. If Unity reports an error, send `build.log` from that output folder to
   the maintainer. Send the folder too if it contains results. If the menu
   never appears or the log is missing, open **Window → General → Console**
   and send the first red error.
6. Later, repeat steps 2–5 in a **separate new 3D (Built-in) project** using
   **Unity 6000.x**, for example **6000.6.9f1**. Keep the 2022.3 project in
   its original Unity version.

URP fixtures will come later as a separate menu item.

If the maintainer needs a manual FBX fallback, use a **CC0-licensed rigged
model**. Exclude TurboSquid and Mixamo assets because their licences forbid
redistributing the rigged assets in the fixtures.
