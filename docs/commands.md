# Commands

| Command                                                                                                                                             | What it does                                                              |
|-----------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------|
| `build-script [build] [<script>]`                                                                                                                   | Builds every module. Modules that did not change are up to date.          |
| `build-script run [--module <name>] [<script>] [-- <args>...]`                                                                                      | Builds, then starts a module. Arguments after `--` go to the program.     |
| `build-script test [--module <name>] [<script>]`                                                                                                    | Builds, then runs the tests of every module, or of one, and sums them up. |
| `build-script new <kind> <path> [--package <package>]`                                                                                              | Creates a project in a new directory.                                     |
| `build-script add <kind> <name> [<script>] [--package <package>]`                                                                                   | Adds a module next to the script and its build call to the script.        |
| `build-script install <group:artifact[:version]> [<script>] [--module <name>] [--compile-only \| --test \| --android-test \| --processor \| --ksp]` | Adds a Maven library to a module. Java, Kotlin and Android only.          |
| `build-script clean [<script>]`                                                                                                                     | Removes the build directories build-script wrote. Downloads stay.         |
| `build-script config [<setting> [<value>]]`, `config --unset <setting>`                                                                             | Shows the settings, one of them, sets or removes one.                     |
| `build-script --help`                                                                                                                               | Shows the commands.                                                       |

> Without a script the commands use `project.build` in the current directory.

## build, run and test

- Modules that do not depend on each other build at the same time, half as many as the machine has cores. `--jobs <n>` sets how many, `--jobs 1` builds one after another.
- A module waits for the modules it depends on. When one fails, the modules that depend on it are not built, and only its error is reported.
- The log lines name their module: `[app] Compiling 3 Kotlin file(s)`.
- `--refresh` asks the Maven repositories again for new versions and SNAPSHOTs, which happens once a day otherwise.
- `--release` builds Android apps for release, only with `build` and `run`. See [android.md](android.md#release-builds).
- `run` starts the last module with a `main_class` or the last Android app, or the one `--module` names. Jars start with the JDK they were built with, in their module directory. Android apps are installed on the device, see [android.md](android.md).
- `test` builds everything first, then runs the tests module by module. A failing module does not stop the others. At the end it lists every module and exits with 1 if one failed:

  ```
  > Tests
    core               passed, 12 tests
    app                FAILED, 2 of 10 tests
    app (androidTest)  passed, 4 tests
    ui                 up to date
  1 failed, 2 passed, 1 up to date
  ```

## Up to date

Every module keeps a fingerprint of its inputs in `build/fingerprints/`: the arguments of its build call, the JDK, its sources and resources, what the modules it depends on built, the newest builds of its SNAPSHOT libraries, and build-script itself.
When nothing of it changed and the outputs exist, the module is not built again. A build that fails half way never counts as up to date.

## new and add

```bash
build-script new kotlin hello          # hello/project.build and hello/src/main/kotlin/...
cd hello && build-script run           # Hello from hello

build-script new multi shop            # a library core and an app that uses it
cd shop
build-script add kotlin-library payments
build-script add android-app shop-android
```

| Project kind | What `new` creates                                              |
|--------------|-----------------------------------------------------------------|
| `java`       | a Java program                                                  |
| `kotlin`     | a Kotlin program                                                |
| `android`    | an Android app with Jetpack Compose                             |
| `multi`      | a Kotlin library `core` and a Kotlin program `app` that uses it |

| Module kind                      | Built with                                                |
|----------------------------------|-----------------------------------------------------------|
| `java-app`, `java-library`       | `build_java`, the app with a `main_class`                 |
| `kotlin-app`, `kotlin-library`   | `build_kotlin`, the app with a `main_class`               |
| `android-app`, `android-library` | `build_android` and `build_android_library`, with Compose |

- Packages start with your domain, reversed: `mvmo.dev` gives `dev.mvmo.<project>`, a module in its own directory `dev.mvmo.<project>.<module>`. `--package` sets the whole package.
- Without a remembered domain, `new` and `add` ask for it and offer to remember it: `y` remembers it, `n` asks again next time, `never` stops offering. Without a terminal, like in CI, they take `example.com`.
- `add` changes the script only where it has to: the `use` the module needs, `google()` for Android modules, and the build call. Libraries go before the first app, apps to the end.
- An added module depends on nothing yet. Add its variable to the dependencies of another module: `list(payments)`.

## install

`install` adds a Maven library to a module, like `cargo add`:

```bash
build-script install com.google.code.gson:gson            # the newest release
build-script install com.google.code.gson:gson:2.10.1     # this version, replaces another one
build-script install org.spigotmc:spigot-api --compile-only --module plugin
```

- Without a version it takes the newest final version the module's repositories have. When there are only milestones, release candidates or SNAPSHOTs, it takes the newest of those and says so.
- The library goes to `dependencies`. `--compile-only`, `--test`, `--android-test`, `--processor` and `--ksp` put it into `compile_only`, `test_dependencies`, `android_test_dependencies`, `annotation_processors` or `ksp`.
- In a script with several modules, `--module` names the one.
- Before the script changes, the library is resolved for the module. Android libraries are refused for Java and Kotlin modules, and libraries that need a newer Java than the module's `jdk`:

  ```
  spring-core-6.2.12.jar needs Java 17, app builds with JDK 11: give it jdk = "17"
  ```

- The script changes only where it has to. The library goes to the end of the `list(...)`, in the style of the list. A list in a variable is changed where the variable is assigned. A missing argument is added to the build call. Another version of the library is replaced.
- A library that is in none of the module's repositories needs its repository in the script first, as `maven(url)`.

## clean

`clean` removes the build directories below the script's directory that build-script wrote: those with `fingerprints/` in them. Build directories of other tools and the downloads stay.

## config and settings

The settings are in the file `settings` next to the downloads, one `name = value` per line.

| Setting           | Meaning                                                                 |
|-------------------|-------------------------------------------------------------------------|
| `domain`          | Your domain, like `mvmo.dev`: the packages of new modules start with it |
| `remember_domain` | `never`: `new` and `add` do not offer to remember the domain            |

```bash
build-script config domain mvmo.dev             # remembers the domain
build-script config --unset remember_domain     # offers to remember again
```

## Where downloads go

Everything build-script downloads is shared by all projects.

| Variable                                             | Effect                                                                                                                                                                                    |
|------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `BUILD_SCRIPT_HOME`                                  | One directory for everything, the caches in `caches/` below, like `~/.gradle`                                                                                                             |
| `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME` | Without `BUILD_SCRIPT_HOME`: JDKs, SDK and emulators in `$XDG_DATA_HOME/build-script`, Maven artifacts in `$XDG_CACHE_HOME/build-script`, the settings in `$XDG_CONFIG_HOME/build-script` |
| neither                                              | `~/.build-script`, the caches in `~/.build-script/caches`                                                                                                                                 |
