// Android builds: resources with aapt2, Kotlin and Java against android.jar, native libraries with CMake, dex with d8, a signed apk
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::common::xml::{self, XmlElement};
use crate::common::{system, up_to_date};
use crate::jvm::jdk::Jdk;
use crate::jvm::maven::{DependencyResolver, MavenCoordinate, TargetPlatform};
use crate::jvm::module::{
    self, Compilation, ModuleBuildInputs, ResolvedDependencies, Sources, create_jar,
};
use crate::language::runtime::{Arguments, BuildScriptContext, BuiltModule};

use super::generated::{
    SymbolTable, binding_class, build_config_java_fields, layouts_by_name, view_binding_source,
    write_r_class,
};
use super::manifest::{
    expand_class_names, merge_manifest, move_aliases_last, read_manifest_with_placeholders,
    remove_entries_marked_for_removal,
};
use super::sdk::{AndroidSdk, SdkCatalog};

pub const DEFAULT_NDK_VERSION: &str = "28.2.13676358";

/// from this API level on Android loads several dex files natively
const NATIVE_MULTIDEX_API: i64 = 21;
/// runtime of generated view bindings, which implement its ViewBinding interface
const VIEW_BINDING_VERSION: &str = "8.1.0";
/// the CMake of the SDK the Android Gradle plugin uses by default
const CMAKE_VERSION: &str = "3.22.1";
/// what the Android Gradle plugin builds without abiFilters
const ABIS: &[&str] = &["armeabi-v7a", "arm64-v8a", "x86", "x86_64"];

/// What aapt2 links: the app, or a library with or without the resources of its libraries
#[derive(Clone, Copy, PartialEq)]
pub enum LinkedResources {
    App,
    LibraryWithItsLibraries,
    LibraryAlone,
}

/// What app and library builds share up to compiling Kotlin
pub struct AndroidBuild {
    pub inputs: ModuleBuildInputs,
    pub namespace: String,
    pub min_sdk: i64,
    pub compile_sdk: i64,
    pub sdk: AndroidSdk,
    pub main_dir: PathBuf,
    /// debug, or debugAndroidTest for the module with its instrumented tests
    variant: &'static str,
    pub build_dir: PathBuf,
    /// own res directories that contain files, lowest priority first
    pub res_dirs: Vec<PathBuf>,
    /// own assets directories that contain files, lowest priority first
    asset_dirs: Vec<PathBuf>,
    aidl_dirs: Vec<PathBuf>,
    /// aidl/ directories of aars, for imports of their parcelables
    aidl_imports: Vec<PathBuf>,
    pub view_binding: bool,
    /// like android.nonTransitiveRClass: R classes have only the package's own resources
    non_transitive_r: bool,
    /// ${name} in manifests, ${applicationId} is always there
    manifest_placeholders: Vec<(String, String)>,
    /// build_config_fields: "Type NAME value"
    build_config_fields: Vec<String>,
    /// compiler, plugins, processors and the libraries
    resolved: ResolvedDependencies,
    /// to compile against and to dex: Maven jars, classes of aars and library modules
    pub jars: Vec<PathBuf>,
    /// of `jars` the classes of library modules, they change with the app
    module_jars: Vec<PathBuf>,
    /// only to compile against
    compile_only: Vec<PathBuf>,
    /// aars and library modules with resources, lowest priority first
    libraries: Vec<AndroidResourceLibrary>,
    /// jniLibs directories and the libraries built with CMake
    native_library_dirs: Vec<PathBuf>,
    cmake_lists: Option<PathBuf>,
    ndk_version: String,
    accept_license: bool,
}

/// desugar_jdk_libs: its d8 configuration and its jars
pub struct DesugaredLibrary {
    config: PathBuf,
    jars: Vec<PathBuf>,
}

/// Like with the Gradle plugin, view bindings come with their runtime, which
/// consumers of a library need as well
pub fn read_android_inputs(
    context: &BuildScriptContext,
    args: &Arguments,
) -> Result<ModuleBuildInputs, String> {
    let mut inputs = ModuleBuildInputs::read(context, args, Sources::KotlinAndJava)?;

    if args.get::<bool>("view_binding")? {
        inputs.dependencies.libraries.push(MavenCoordinate::new(
            "androidx.databinding",
            "viewbinding",
            VIEW_BINDING_VERSION,
        ));
    }

    return Ok(inputs);
}

/// The module's directories of the list argument `name` that contain files, lowest priority first
pub fn existing_dirs(
    inputs: &ModuleBuildInputs,
    args: &Arguments,
    name: &str,
) -> Result<Vec<PathBuf>, String> {
    let dirs: Vec<String> = args.get(name)?;
    return Ok(dirs
        .iter()
        .map(|dir| inputs.directory.join(dir))
        .filter(|dir| system::contains_files(dir))
        .collect());
}

/// The module's jniLibs and, with a CMakeLists.txt, the libraries built from it
pub fn native_library_dirs(
    inputs: &ModuleBuildInputs,
    args: &Arguments,
) -> Result<Vec<PathBuf>, String> {
    let mut dirs = source_set_dirs_with_files(inputs, "jniLibs");

    if args.get::<Option<String>>("cmake")?.is_some() {
        dirs.push(CMakeBuild::libraries_dir(&cmake_dir(inputs)));
    }

    return Ok(dirs);
}

/// What the outputs of an Android module depend on besides the sources:
/// resources, AIDL, manifests, assets, native libraries, Java resources
pub fn android_input_paths(
    inputs: &ModuleBuildInputs,
    args: &Arguments,
) -> Result<Vec<PathBuf>, String> {
    let mut paths: Vec<PathBuf> = ["res_dirs", "aidl_dirs", "asset_dirs"]
        .iter()
        .map(|name| args.get::<Vec<String>>(name))
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .flatten()
        .map(|dir| inputs.directory.join(dir))
        .collect();

    for source_set in ["src/main", "src/debug"] {
        paths.push(
            inputs
                .directory
                .join(source_set)
                .join("AndroidManifest.xml"),
        );
        paths.push(inputs.directory.join(source_set).join("jniLibs"));
    }

    paths.extend(inputs.resources.inputs());

    // native sources, where Android Studio keeps them
    if let Some(cmake) = args.get::<Option<String>>("cmake")? {
        paths.push(inputs.directory.join(cmake));
        paths.push(inputs.directory.join("src/main/cpp"));
    }

    return Ok(paths);
}

/// Non-class files of the jars, like the Gradle plugin packages them: the
/// first jar wins, META-INF/services files are concatenated
pub fn merge_java_resources(jars: &[PathBuf], output: &Path, scratch: &Path) -> Result<(), String> {
    system::recreate_dir(output)?;
    // jars of the cache never change, like those of Maven and of aars: their resources are extracted once
    let cache = system::cache_dir("");
    for jar in unique(jars) {
        let resources = if jar.starts_with(&cache) {
            system::kept_in_cache("java-resources", &jar, |target| {
                extract_java_resources(&jar, target)
            })?
        } else {
            extract_java_resources(&jar, scratch)?;
            scratch.to_path_buf()
        };

        for path in system::files_below(&[resources.clone()]) {
            let relative = path
                .strip_prefix(&resources)
                .map_err(|error| error.to_string())?;

            let target = output.join(relative);

            let is_service = relative.starts_with("META-INF/services");
            if target.exists() && !is_service {
                continue;
            }

            let mut content = fs::read(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;

            if target.exists() {
                let mut existing = fs::read(&target)
                    .map_err(|error| format!("could not read {}: {error}", target.display()))?;

                if !existing.ends_with(b"\n") {
                    existing.push(b'\n');
                }

                existing.extend(content);
                content = existing;
            }

            system::create_parent_dir(&target)?;
            fs::write(&target, content)
                .map_err(|error| format!("could not write {}: {error}", target.display()))?;
        }
    }

    return system::remove_dir(scratch);
}

impl AndroidBuild {
    pub fn prepare(inputs: ModuleBuildInputs, args: &Arguments) -> Result<Self, String> {
        let build_dir = inputs.directory.join("build");
        return Self::prepare_variant(inputs, args, "debug", build_dir);
    }

    /// The module with its instrumented tests, built apart from the module: the
    /// resources and assets of src/androidTest lie over those of the module
    pub fn prepare_android_tests(
        inputs: ModuleBuildInputs,
        args: &Arguments,
    ) -> Result<Self, String> {
        let build_dir = inputs.directory.join("build/androidTest");
        let mut build = Self::prepare_variant(inputs, args, "debugAndroidTest", build_dir)?;
        let test_dir = build.inputs.directory.join("src/androidTest");
        build.res_dirs.extend(
            [test_dir.join("res")]
                .into_iter()
                .filter(|dir| system::contains_files(dir)),
        );
        build.asset_dirs.extend(
            [test_dir.join("assets")]
                .into_iter()
                .filter(|dir| system::contains_files(dir)),
        );

        return Ok(build);
    }

    fn prepare_variant(
        inputs: ModuleBuildInputs,
        args: &Arguments,
        variant: &'static str,
        build_dir: PathBuf,
    ) -> Result<Self, String> {
        let compile_sdk: i64 = args.get("compile_sdk")?;
        let build_tools: String = args.get("build_tools")?;
        let accept_license: bool = args.get("accept_sdk_license")?;
        let build_config_fields: Vec<String> = args.get("build_config_fields")?;
        // checked before the build starts
        build_config_java_fields(&build_config_fields)?;

        let manifest_placeholders = args
            .assignments("manifest_placeholders")?
            .into_iter()
            .map(|(name, value)| (name, value.trim().to_string()))
            .collect();

        for module in BuiltModule::transitive(&inputs.dependencies.modules) {
            if module.jvm().classes.is_none() {
                return Err(format!(
                    "{} is an Android app and cannot be a dependency",
                    module.name
                ));
            }
        }

        let sdk = AndroidSdk::ensure(compile_sdk, &build_tools, accept_license)?;
        system::log(
            &inputs.name,
            &format!(
                "Android SDK {} (android-{compile_sdk}, build-tools {build_tools})",
                sdk.root.display(),
            ),
        );
        system::log(&inputs.name, &format!("JDK {}", inputs.jdk.home.display()));

        let resolved = inputs.resolve(TargetPlatform::Android)?;
        let unpacked = UnpackedLibraries::unpack(&resolved, &sdk, &build_dir)?;

        return Ok(AndroidBuild {
            namespace: args.get("namespace")?,
            min_sdk: args.get("min_sdk")?,
            compile_sdk,
            sdk,
            main_dir: inputs.directory.join("src/main"),
            variant,
            build_dir,
            res_dirs: existing_dirs(&inputs, args, "res_dirs")?,
            asset_dirs: existing_dirs(&inputs, args, "asset_dirs")?,
            aidl_dirs: args
                .get::<Vec<String>>("aidl_dirs")?
                .iter()
                .map(|dir| inputs.directory.join(dir))
                .collect(),
            aidl_imports: unpacked.aidl_imports,
            view_binding: args.get("view_binding")?,
            non_transitive_r: args.get("non_transitive_r_class")?,
            manifest_placeholders,
            build_config_fields,
            resolved,
            jars: unpacked.jars,
            module_jars: unpacked.module_jars,
            compile_only: unpacked.compile_only,
            libraries: unpacked.libraries,
            native_library_dirs: native_library_dirs(&inputs, args)?,
            cmake_lists: args
                .get::<Option<String>>("cmake")?
                .map(|cmake| inputs.directory.join(cmake)),
            ndk_version: args.get("ndk_version")?,
            accept_license,
            inputs,
        });
    }

    pub fn compile_own_resources(&self) -> Result<Vec<PathBuf>, String> {
        let mut outputs = vec![];

        for (index, res_dir) in self.res_dirs.iter().enumerate() {
            let name = format!("{}-own-{index}", self.inputs.name);
            outputs.push(compiled_resources(
                &self.sdk,
                res_dir,
                &self.build_dir.join("intermediates/res"),
                &name,
            )?);
        }

        return Ok(outputs);
    }

    /// The manifest for aapt2: with package, and for apps merged with the
    /// manifests of all libraries. `overlay` lies over the module's manifest, like
    /// the one of instrumented tests.
    pub fn write_manifest(
        &self,
        source: Option<&Path>,
        overlay: Option<&XmlElement>,
        application_id: &str,
        merge: bool,
        output: &Path,
    ) -> Result<XmlElement, String> {
        let mut manifest = match source {
            Some(source) => read_manifest_with_placeholders(
                source,
                application_id,
                &self.manifest_placeholders,
            )?,
            None => xml::parse(
                r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"/>"#,
            )?,
        };

        // the debug source set wins over main
        let debug_manifest = self.inputs.directory.join("src/debug/AndroidManifest.xml");
        if debug_manifest.is_file() {
            let mut debug = read_manifest_with_placeholders(
                &debug_manifest,
                application_id,
                &self.manifest_placeholders,
            )?;

            for (key, value) in &manifest.attributes {
                if debug.attribute(key).is_none() {
                    debug.attributes.push((key.clone(), value.clone()));
                }
            }

            merge_manifest(&mut debug, &manifest);
            manifest = debug;
        }

        if let Some(overlay) = overlay {
            merge_manifest(&mut manifest, overlay);
        }

        match manifest.attribute("package") {
            Some(package) if package != self.namespace => {
                return Err(format!(
                    "the manifest declares package {package}, but the namespace is {}",
                    self.namespace
                ));
            }
            Some(_) => {}
            None => manifest
                .attributes
                .push(("package".to_string(), self.namespace.clone())),
        }

        if merge {
            for library in &self.libraries {
                let Some(path) = &library.manifest else {
                    continue;
                };

                let mut library_manifest = read_manifest_with_placeholders(
                    path,
                    application_id,
                    &self.manifest_placeholders,
                )?;

                expand_class_names(&mut library_manifest, &library.namespace);
                merge_manifest(&mut manifest, &library_manifest);
            }

            remove_entries_marked_for_removal(&mut manifest);
        }

        // min_sdk and target_sdk come from the build, like with the Gradle plugin
        manifest.children.retain(|child| child.name != "uses-sdk");
        move_aliases_last(&mut manifest);

        let content = xml::write(&manifest, &|attribute| {
            attribute.starts_with("tools:") || attribute == "xmlns:tools"
        });

        system::write_file(output, &content)?;

        return Ok(manifest);
    }

    /// The symbols of a library in `symbols`, to write its R classes from. Like
    /// the Gradle plugin, libraries may use resources that only the app has,
    /// aapt2 checks references when it links the app.
    pub fn link_library(
        &self,
        manifest: &Path,
        own: &[PathBuf],
        work_dir: &Path,
        symbols: &Path,
    ) -> Result<(), String> {
        let with_libraries = work_dir.join("R-with-libraries.txt");
        self.link(
            manifest,
            own,
            LinkedResources::LibraryWithItsLibraries,
            &work_dir.join("library.ap_"),
            &with_libraries,
            &[],
        )?;

        if !self.non_transitive_r {
            return system::copy_file(&with_libraries, symbols);
        }

        let own_only = work_dir.join("R-own.txt");
        self.link(
            manifest,
            own,
            LinkedResources::LibraryAlone,
            &work_dir.join("library-own.ap_"),
            &own_only,
            &[],
        )?;

        return SymbolTable::write_own(
            &SymbolTable::read(&with_libraries)?,
            &SymbolTable::read(&own_only)?,
            symbols,
        );
    }

    /// For the module (with `own_symbols`) and every library that has one,
    /// each with the resources it declares. With `ids`, the app's symbols,
    /// they are final and get the app's ids, otherwise they are for compiling
    /// against.
    pub fn write_r_classes(
        &self,
        own_symbols: &Path,
        ids: Option<&SymbolTable>,
        output: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        system::remove_dir(output)?;
        let mut packages: Vec<(&str, &Path)> = vec![(&self.namespace, own_symbols)];

        for library in &self.libraries {
            if let Some(symbols) = &library.symbols {
                if !packages
                    .iter()
                    .any(|(package, _)| *package == library.namespace)
                {
                    packages.push((&library.namespace, symbols));
                }
            }
        }

        let mut files = vec![];

        for (package, symbols) in packages {
            files.push(write_r_class(
                package,
                &SymbolTable::read(symbols)?,
                ids,
                output,
            )?);
        }

        return Ok(files);
    }

    /// Of a debug build in the namespace, like the Gradle plugin writes it:
    /// `fields` ("Type NAME value"), then build_config_fields
    pub fn write_build_config(&self, fields: &[String]) -> Result<PathBuf, String> {
        let mut declarations = vec![
            "boolean DEBUG true".to_string(),
            "String BUILD_TYPE debug".to_string(),
        ];

        declarations.extend(fields.iter().cloned());
        declarations.extend(self.build_config_fields.iter().cloned());
        let content = format!(
            "package {};\n\npublic final class BuildConfig {{\n{}}}\n",
            self.namespace,
            build_config_java_fields(&declarations)?,
        );

        let file = self
            .build_dir
            .join("intermediates/generated/buildconfig")
            .join(self.namespace.replace('.', "/"))
            .join("BuildConfig.java");

        system::write_file(&file, &content)?;

        return Ok(file);
    }

    /// Of the module and then of its libraries, highest priority first as
    /// aapt2 takes a file from the first directory that has it. Like the
    /// Gradle plugin merges them: debug over main over the libraries.
    pub fn merged_asset_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = self.asset_dirs.clone();
        dirs.reverse();
        dirs.extend(
            self.libraries
                .iter()
                .rev()
                .flat_map(|library| library.asset_dirs.iter().cloned()),
        );

        return dirs;
    }

    /// From the module's CMakeLists.txt, if it has one
    pub fn build_native_libraries(&self) -> Result<(), String> {
        let Some(cmake_lists) = &self.cmake_lists else {
            return Ok(());
        };
        return CMakeBuild {
            cmake_lists,
            sdk_root: &self.sdk.root,
            ndk_version: &self.ndk_version,
            min_sdk: self.min_sdk,
            work_dir: cmake_dir(&self.inputs),
            accept_license: self.accept_license,
        }
        .run(&|message| system::log(&self.inputs.name, message));
    }

    /// Those of the libraries and of the module to `output`/lib/<abi>, the module's win
    pub fn merge_native_libraries(&self, output: &Path) -> Result<(), String> {
        system::remove_dir(output)?;

        let mut dirs: Vec<PathBuf> = self
            .libraries
            .iter()
            .flat_map(|library| library.native_library_dirs.iter().cloned())
            .collect();

        dirs.extend(self.native_library_dirs.iter().cloned());
        for dir in dirs {
            for library in system::files_with_extension(&[dir.clone()], "so") {
                let relative = library
                    .strip_prefix(&dir)
                    .map_err(|error| error.to_string())?;

                system::copy_file(&library, &output.join("lib").join(relative))?;
            }
        }

        return Ok(());
    }

    /// R and BuildConfig
    pub fn compile_generated_java(
        &self,
        sources: &[PathBuf],
        classpath: &[PathBuf],
        output: &Path,
    ) -> Result<(), String> {
        system::remove_dir(output)?;
        return module::compile_java(&self.inputs.jdk, sources, classpath, output, "11", None);
    }

    /// android.jar, the given class directories and all library jars
    pub fn android_classpath(&self, extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut classpath = vec![self.sdk.android_jar.clone()];
        classpath.extend(extra.iter().cloned());
        classpath.extend(self.jars.iter().cloned());
        classpath.extend(self.compile_only.iter().cloned());

        return classpath;
    }

    /// Kotlin and Java, the view bindings and AIDL interfaces, what the symbol
    /// processors generate. `generated` are class directories to compile
    /// against, like R and BuildConfig. Returns the directories of Java
    /// resources the symbol processors wrote.
    pub fn compile_sources(
        &self,
        generated: &[PathBuf],
        output: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        // bindings may use the module's own views, AIDL interfaces its classes.
        // The symbol processors see them too, they skip classes whose signatures
        // use types they cannot resolve, like bindings.
        let mut source_dirs = self.inputs.source_dirs.clone();
        source_dirs.push(self.generate_view_bindings()?);
        source_dirs.push(self.generate_aidl()?);

        return Compilation {
            inputs: &self.inputs,
            tools: &self.resolved,
            variant: self.variant,
            source_dirs: &source_dirs,
            classpath: &self.android_classpath(generated),
            friends: &[],
            symbol_processors: self.resolved.symbol_processors.as_ref(),
            annotation_processors: &self.resolved.annotation_processor_path,
            target: TargetPlatform::Android,
            output,
        }
        .run();
    }

    pub fn jar(&self, directory: &Path, jar: &Path) -> Result<PathBuf, String> {
        create_jar(&self.inputs.jdk, jar, None, &[directory])?;
        return Ok(jar.to_path_buf());
    }

    /// desugar_jdk_libs and its d8 configuration, like isCoreLibraryDesugaringEnabled
    pub fn desugared_library(
        &self,
        version: &str,
        work_dir: &Path,
    ) -> Result<DesugaredLibrary, String> {
        let mut resolver =
            DependencyResolver::new(self.inputs.repositories.clone(), TargetPlatform::Jvm);

        let jars = resolver.resolve_with_platforms(
            &[MavenCoordinate::new(
                "com.android.tools",
                "desugar_jdk_libs",
                version,
            )],
            &[],
        )?;

        let configuration_jar = jars
            .iter()
            .find(|jar| {
                jar.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("desugar_jdk_libs_configuration")
                })
            })
            .ok_or("desugar_jdk_libs came without its configuration")?;

        let configuration =
            system::read_zip_entry(configuration_jar, "META-INF/desugar/d8/desugar.json")?;

        let config = work_dir.join("desugar.json");
        system::write_file(&config, &String::from_utf8_lossy(&configuration))?;

        return Ok(DesugaredLibrary { config, jars });
    }

    /// With L8, added after the app's dex files. Kept in `work_dir` until the
    /// library or how it is dexed changes, it is the same for every build.
    pub fn dex_desugared_library(
        &self,
        library: &DesugaredLibrary,
        dex_dir: &Path,
        work_dir: &Path,
    ) -> Result<(), String> {
        let mut hasher = DefaultHasher::new();
        (
            &self.sdk.d8,
            &self.sdk.android_jar,
            self.min_api(),
            up_to_date::files_hash(&library.jars)?,
            system::read_file(&library.config)?,
        )
            .hash(&mut hasher);
        let dexed = work_dir.join(format!("{:016x}", hasher.finish()));

        if !dexed.is_dir() {
            system::recreate_dir(work_dir)?;
            let partial = work_dir.join("partial");

            let mut command = self
                .inputs
                .jdk
                .java_command(&[self.sdk.d8.clone()], "com.android.tools.r8.L8")?;

            command
                .arg("--debug")
                .arg("--min-api")
                .arg(self.min_api().to_string())
                .arg("--lib")
                .arg(&self.sdk.android_jar)
                .arg("--desugared-lib")
                .arg(&library.config)
                .arg("--output")
                .arg(&partial)
                .args(&library.jars);
            system::create_dir(&partial)?;
            system::run_command(&mut command, "L8")?;
            fs::rename(&partial, &dexed)
                .map_err(|error| format!("could not move {}: {error}", partial.display()))?;
        }

        let mut next = system::files_with_extension(&[dex_dir.to_path_buf()], "dex").len() + 1;
        for dex in dex_files(&dexed) {
            system::copy_file(&dex, &dex_dir.join(format!("classes{next}.dex")))?;
            next += 1;
        }

        return Ok(());
    }

    /// The own classes and those of the library modules on every build, the other
    /// libraries once: like the Gradle plugin dexes them ahead, their dex files are
    /// kept until one of them changes. Native multidex loads the dex files of both.
    pub fn dex(
        &self,
        own: &[PathBuf],
        output: &Path,
        desugared: Option<&DesugaredLibrary>,
    ) -> Result<(), String> {
        // like Android Studio does for debug builds: below API 21 one dex file
        // would have to hold everything, so dex for native multidex instead
        if self.min_api() != self.min_sdk {
            system::log(
                &self.inputs.name,
                &format!(
                    "Note: min_sdk {} is below {NATIVE_MULTIDEX_API}, the debug apk uses native multidex and needs Android 5.0 or newer",
                    self.min_sdk,
                ),
            );
        }

        let modules: HashSet<&PathBuf> = self.module_jars.iter().collect();
        let own = unique(own.iter().chain(&self.module_jars));
        let libraries = unique(self.jars.iter().filter(|jar| !modules.contains(jar)));

        let libraries_dex = self.dexed_libraries(&libraries, desugared)?;
        let own_dex = self.build_dir.join("intermediates/dex-own");
        system::recreate_dir(&own_dex)?;
        self.run_d8(&own, &libraries, &own_dex, desugared)?;

        // the own classes first, in classes.dex
        system::recreate_dir(output)?;
        for (index, dex) in dex_files(&own_dex)
            .into_iter()
            .chain(dex_files(&libraries_dex))
            .enumerate()
        {
            let name = if index == 0 {
                "classes.dex".to_string()
            } else {
                format!("classes{}.dex", index + 1)
            };

            system::copy_file(&dex, &output.join(name))?;
        }

        return Ok(());
    }

    /// In intermediates/dex-libraries/<hash of the libraries and of how d8 runs>
    fn dexed_libraries(
        &self,
        libraries: &[PathBuf],
        desugared: Option<&DesugaredLibrary>,
    ) -> Result<PathBuf, String> {
        let mut hasher = DefaultHasher::new();
        (
            &self.sdk.d8,
            &self.sdk.android_jar,
            self.min_api(),
            up_to_date::files_hash(libraries)?,
        )
            .hash(&mut hasher);

        if let Some(desugared) = desugared {
            system::read_file(&desugared.config)?.hash(&mut hasher);
        }

        let dexed = self.build_dir.join("intermediates/dex-libraries");
        let dex = dexed.join(format!("{:016x}", hasher.finish()));

        if dex.is_dir() {
            system::log(&self.inputs.name, "Libraries dexed before");
            return Ok(dex);
        }

        // only those of the current libraries are kept, renamed once complete
        system::recreate_dir(&dexed)?;
        let partial = dexed.join("partial");
        system::create_dir(&partial)?;

        if !libraries.is_empty() {
            system::log(
                &self.inputs.name,
                &format!("Dexing {} libraries, again only when they change", libraries.len()),
            );
            self.run_d8(libraries, &[], &partial, desugared)?;
        }

        fs::rename(&partial, &dex)
            .map_err(|error| format!("could not move {}: {error}", partial.display()))?;

        return Ok(dex);
    }

    /// `classpath` only to desugar against, like the libraries for the own classes
    fn run_d8(
        &self,
        inputs: &[PathBuf],
        classpath: &[PathBuf],
        output: &Path,
        desugared: Option<&DesugaredLibrary>,
    ) -> Result<(), String> {
        let mut command = self
            .inputs
            .jdk
            .java_command(&[self.sdk.d8.clone()], "com.android.tools.r8.D8")?;

        command
            .arg("--debug")
            .arg("--min-api")
            .arg(self.min_api().to_string())
            .arg("--lib")
            .arg(&self.sdk.android_jar)
            .arg("--output")
            .arg(output);

        for jar in classpath {
            command.arg("--classpath").arg(jar);
        }

        if let Some(desugared) = desugared {
            command.arg("--desugared-lib").arg(&desugared.config);
        }

        command.args(inputs);

        let result = command
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("could not start d8: {error}"))?;

        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );

        if !result.status.success() {
            eprint!("{text}");
            return Err(format!("d8 failed ({})", result.status));
        }

        let (shown, hidden) = split_d8_noise(&text);

        for line in shown {
            eprintln!("{line}");
        }

        if hidden > 0 {
            system::log(
                &self.inputs.name,
                &format!(
                    "d8: hid {hidden} lines of notes about libraries (Kotlin metadata newer than this d8 knows, stripped debug info)",
                ),
            );
        }

        return Ok(());
    }

    /// Resources, dex, Java resources and native libraries, aligned and signed with the debug key
    pub fn package(
        &self,
        linked: &Path,
        dex_dir: &Path,
        java_resources: &Path,
        native_libraries: &Path,
        work_dir: &Path,
        apk: &Path,
    ) -> Result<(), String> {
        let unaligned = work_dir.join("unaligned.apk");
        let aligned = work_dir.join("aligned.apk");
        system::copy_file(linked, &unaligned)?;

        let mut zip = Command::new("zip");
        zip.args(["-q", "-j"])
            .arg(&unaligned)
            .args(system::files_with_extension(
                &[dex_dir.to_path_buf()],
                "dex",
            ));
        system::run_command(&mut zip, "adding dex files")?;

        if system::contains_files(java_resources) {
            let unaligned = fs::canonicalize(&unaligned).map_err(|error| error.to_string())?;
            let mut zip = Command::new("zip");
            zip.args(["-q", "-r"])
                .arg(&unaligned)
                .arg(".")
                .current_dir(java_resources);
            system::run_command(&mut zip, "adding java resources")?;
        }

        // stored and page aligned, so they load from the apk and extractNativeLibs="false" works
        if system::contains_files(native_libraries) {
            let unaligned = fs::canonicalize(&unaligned).map_err(|error| error.to_string())?;
            let mut zip = Command::new("zip");
            zip.args(["-q", "-r", "-0"])
                .arg(&unaligned)
                .arg("lib")
                .current_dir(native_libraries);
            system::run_command(&mut zip, "adding native libraries")?;
        }

        let _ = fs::remove_file(&aligned);
        let mut zipalign = Command::new(&self.sdk.zipalign);

        // 16 KB pages like Android 15 devices may have, older zipaligns only know 4 KB
        match self
            .sdk
            .build_tools
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
        {
            Some(major) if major >= 35 => zipalign.args(["-P", "16"]),
            _ => zipalign.arg("-p"),
        };

        zipalign.args(["-f", "4"]).arg(&unaligned).arg(&aligned);
        system::run_command(&mut zipalign, "zipalign")?;

        system::create_parent_dir(apk)?;
        let keystore = debug_keystore(&self.inputs.jdk)?;
        let mut sign = Command::new(self.inputs.jdk.tool_path("java"));
        sign.arg("-jar")
            .arg(&self.sdk.apksigner)
            .arg("sign")
            .arg("--ks")
            .arg(&keystore)
            // the well known password of the debug key every Android tool uses
            .args([
                "--ks-pass",
                "pass:android",
                "--key-pass",
                "pass:android",
                "--ks-key-alias",
                "androiddebugkey",
            ])
            .arg("--min-sdk-version")
            .arg(self.min_sdk.to_string())
            .arg("--out")
            .arg(apk)
            .arg(&aligned);
        system::run_command(&mut sign, "apksigner sign")?;

        // a sanity check, its warnings about META-INF files outside the v2/v3 signature are expected
        let verify = Command::new(self.inputs.jdk.tool_path("java"))
            .arg("-jar")
            .arg(&self.sdk.apksigner)
            .arg("verify")
            .arg(apk)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("could not start apksigner: {error}"))?;

        if !verify.status.success() {
            eprint!(
                "{}{}",
                String::from_utf8_lossy(&verify.stdout),
                String::from_utf8_lossy(&verify.stderr)
            );
            return Err(format!("apksigner verify failed ({})", verify.status));
        }

        return Ok(());
    }

    /// aapt2 link of the module's resources `own`, the ids in `symbols`
    pub fn link(
        &self,
        manifest: &Path,
        own: &[PathBuf],
        resources: LinkedResources,
        output: &Path,
        symbols: &Path,
        extra: &[String],
    ) -> Result<(), String> {
        system::create_parent_dir(output)?;
        system::create_parent_dir(symbols)?;

        let mut command = Command::new(&self.sdk.aapt2);
        command
            .arg("link")
            .arg("-I")
            .arg(&self.sdk.android_jar)
            .arg("--manifest")
            .arg(manifest)
            .arg("-o")
            .arg(output)
            .arg("--output-text-symbols")
            .arg(symbols)
            .arg("--auto-add-overlay")
            .arg("--min-sdk-version")
            .arg(self.min_sdk.to_string())
            .args(extra);

        if resources != LinkedResources::App {
            command.arg("--non-final-ids");
            // aapt2 links stubs of the framework itself only as an app
            if self.namespace != "android" {
                command.args(["--static-lib", "--merge-only"]);
            }
        }

        // later overlays win: libraries, then the module itself
        let libraries = if resources == LinkedResources::LibraryAlone {
            &[][..]
        } else {
            &self.libraries[..]
        };

        for compiled in libraries
            .iter()
            .flat_map(|library| &library.compiled)
            .chain(own)
        {
            command.arg("-R").arg(compiled);
        }

        return system::run_command(&mut command, "aapt2 link");
    }

    /// Java interfaces for the .aidl files, like the Gradle plugin's aidl task.
    /// Returns the directory they are in.
    fn generate_aidl(&self) -> Result<PathBuf, String> {
        let output = self.build_dir.join("intermediates/generated/aidl");
        system::remove_dir(&output)?;
        let files = system::files_with_extension(&self.aidl_dirs, "aidl");

        if files.is_empty() {
            return Ok(output);
        }

        system::log(
            &self.inputs.name,
            &format!("Generating Java for {} AIDL file(s)", files.len()),
        );
        for file in &files {
            let mut command = Command::new(&self.sdk.aidl);
            command.arg(format!("-p{}", self.sdk.framework_aidl.display()));

            for include in self.aidl_dirs.iter().chain(&self.aidl_imports) {
                command.arg(format!("-I{}", include.display()));
            }

            command.arg(format!("-o{}", output.display())).arg(file);
            system::run_command(&mut command, &format!("aidl {}", file.display()))?;
        }

        return Ok(output);
    }

    /// <Layout>Binding classes for the own layouts, like view_binding = true.
    /// Returns the directory they are in.
    fn generate_view_bindings(&self) -> Result<PathBuf, String> {
        let output = self.build_dir.join("intermediates/generated/viewbinding");
        system::remove_dir(&output)?;

        if !self.view_binding {
            return Ok(output);
        }

        let layouts = layouts_by_name(&self.res_dirs)?;
        // <include>s may bind layouts of library modules with their classes,
        // searched from the last module on, which wins like for resources
        let mut libraries = vec![];

        for module in BuiltModule::transitive(&self.inputs.dependencies.modules)
            .iter()
            .rev()
        {
            if let Some(android) = module
                .jvm()
                .android
                .as_ref()
                .filter(|android| android.view_binding)
            {
                libraries.push((
                    android.namespace.clone(),
                    layouts_by_name(&android.res_dirs)?,
                ));
            }
        }

        let directory = output
            .join(self.namespace.replace('.', "/"))
            .join("databinding");

        for (layout, variants) in &layouts {
            if let Some(source) =
                view_binding_source(&self.namespace, layout, variants, &layouts, &libraries)
            {
                system::write_file(
                    &directory.join(format!("{}.java", binding_class(layout))),
                    &source,
                )?;
            }
        }

        return Ok(output);
    }

    /// API level the dex files are made for
    fn min_api(&self) -> i64 {
        return self.min_sdk.max(NATIVE_MULTIDEX_API);
    }
}

/// The non-class files of a jar, without its manifest and signatures
fn extract_java_resources(jar: &Path, target: &Path) -> Result<(), String> {
    system::recreate_dir(target)?;
    let mut unzip = Command::new("unzip");
    unzip
        .stdin(Stdio::null())
        .args(["-q", "-o"])
        .arg(jar)
        .args([
            "-x",
            "*.class",
            "META-INF/MANIFEST.MF",
            "META-INF/*.SF",
            "META-INF/*.RSA",
            "META-INF/*.DSA",
            "META-INF/*.EC",
            "META-INF/versions/*",
        ])
        .arg("-d")
        .arg(target);

    // exit code 1 = only warnings (an exclude pattern did not match), 11 = nothing left to extract
    let result = unzip
        .output()
        .map_err(|error| format!("could not start unzip: {error}"))?;

    if !matches!(result.status.code(), Some(0 | 1 | 11)) {
        return Err(format!(
            "unzip of {} failed ({}): {}",
            jar.display(),
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }

    system::make_accessible(target);

    return Ok(());
}

/// Each path once, in the order they come
fn unique<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    return paths
        .into_iter()
        .filter(|path| seen.insert(*path))
        .cloned()
        .collect();
}

/// classes.dex, classes2.dex, ... of a d8 output, in that order
fn dex_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = system::files_with_extension(&[directory.to_path_buf()], "dex");
    files.sort_by_key(|file| {
        let stem = file
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        stem.trim_start_matches("classes")
            .parse::<usize>()
            .unwrap_or(1)
    });

    return files;
}

/// Resources of an aar or a library module
struct AndroidResourceLibrary {
    namespace: String,
    /// outputs of aapt2 compile, empty without resources
    compiled: Vec<PathBuf>,
    manifest: Option<PathBuf>,
    /// R.txt: what its R class has
    symbols: Option<PathBuf>,
    asset_dirs: Vec<PathBuf>,
    /// directories with <abi>/*.so: jni/ of aars, jniLibs/ of modules
    native_library_dirs: Vec<PathBuf>,
}

/// What the libraries of a module bring: classes to compile against and to
/// dex, resources to link, parcelables for AIDL
struct UnpackedLibraries {
    jars: Vec<PathBuf>,
    /// of `jars` those of library modules
    module_jars: Vec<PathBuf>,
    compile_only: Vec<PathBuf>,
    aidl_imports: Vec<PathBuf>,
    /// aars and library modules with resources, lowest priority first
    libraries: Vec<AndroidResourceLibrary>,
}

impl UnpackedLibraries {
    fn unpack(
        resolved: &ResolvedDependencies,
        sdk: &AndroidSdk,
        build_dir: &Path,
    ) -> Result<Self, String> {
        let mut unpacked = UnpackedLibraries {
            jars: vec![],
            module_jars: vec![],
            compile_only: vec![],
            aidl_imports: vec![],
            libraries: vec![],
        };

        let mut aars = vec![];

        for file in &resolved.libraries {
            if is_aar(file) {
                aars.push(unpacked.unpack_aar(file, sdk)?);
            } else {
                unpacked.jars.push(file.clone());
            }
        }

        // resolution order is direct dependencies first, overlays need the reverse
        aars.reverse();
        unpacked.libraries.extend(aars);

        for module in &resolved.modules {
            let output = module.jvm();
            unpacked.jars.extend(output.classes.iter().cloned());
            unpacked.module_jars.extend(output.classes.iter().cloned());

            let Some(android) = &output.android else {
                continue;
            };

            let mut compiled = vec![];

            for (index, res_dir) in android.res_dirs.iter().enumerate() {
                compiled.push(compiled_resources(
                    sdk,
                    res_dir,
                    &build_dir.join("intermediates/res"),
                    &format!("{}-{index}", module.name),
                )?);
            }

            unpacked.libraries.push(AndroidResourceLibrary {
                namespace: android.namespace.clone(),
                compiled,
                manifest: android.manifest.clone(),
                symbols: android.symbols.clone(),
                asset_dirs: android.asset_dirs.clone(),
                native_library_dirs: android.native_library_dirs.clone(),
            });
        }

        // compile only aars contribute their classes, not their resources
        for file in &resolved.compile_only {
            if is_aar(file) {
                unpacked.compile_only.extend(system::files_with_extension(
                    &[system::kept_in_cache("extracted", file, |target| system::extract(file, target))?],
                    "jar",
                ));
            } else {
                unpacked.compile_only.push(file.clone());
            }
        }

        return Ok(unpacked);
    }

    /// Extracted once into the cache, a released aar never changes
    fn unpack_aar(
        &mut self,
        aar: &Path,
        sdk: &AndroidSdk,
    ) -> Result<AndroidResourceLibrary, String> {
        let stem = aar
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let target = system::kept_in_cache("extracted", aar, |target| system::extract(aar, target))?;

        let classes = target.join("classes.jar");

        if classes.is_file() {
            self.jars.push(classes);
        }

        if target.join("aidl").is_dir() {
            self.aidl_imports.push(target.join("aidl"));
        }

        self.jars
            .extend(system::files_with_extension(&[target.join("libs")], "jar"));

        let manifest = target.join("AndroidManifest.xml");

        let namespace = xml::parse(&system::read_file(&manifest)?)
            .map_err(|error| format!("{}: {error}", manifest.display()))?
            .attribute("package")
            .ok_or_else(|| format!("{} has no package", manifest.display()))?
            .to_string();

        // compiled resources of a released aar never change
        let res_dir = target.join("res");
        let mut compiled = vec![];

        if system::contains_files(&res_dir) {
            let mut hasher = DefaultHasher::new();
            aar.hash(&mut hasher);
            let cached = system::cache_dir("android/res")
                .join(&sdk.build_tools)
                .join(format!("{stem}-{:016x}.zip", hasher.finish()));

            if !cached.is_file() {
                compile_resources(&sdk.aapt2, &res_dir, &cached)?;
            }

            compiled.push(cached);
        }

        return Ok(AndroidResourceLibrary {
            namespace,
            compiled,
            manifest: Some(manifest),
            symbols: Some(target.join("R.txt")).filter(|symbols| symbols.is_file()),
            asset_dirs: [target.join("assets")]
                .into_iter()
                .filter(|dir| system::contains_files(dir))
                .collect(),
            native_library_dirs: [target.join("jni")]
                .into_iter()
                .filter(|dir| system::contains_files(dir))
                .collect(),
        });
    }
}

/// The shared libraries of one CMakeLists.txt, like externalNativeBuild in the Android Gradle plugin
struct CMakeBuild<'a> {
    cmake_lists: &'a Path,
    sdk_root: &'a Path,
    ndk_version: &'a str,
    min_sdk: i64,
    /// build/intermediates/cmake: the CMake builds and the libraries in libs/<abi>
    work_dir: PathBuf,
    accept_license: bool,
}

impl CMakeBuild<'_> {
    /// One directory per ABI like jniLibs
    fn libraries_dir(work_dir: &Path) -> PathBuf {
        return work_dir.join("libs");
    }

    /// Installs the NDK and CMake if needed, then configures and builds the
    /// libraries for every ABI, a debug build like the Gradle plugin's
    fn run(&self, log: &dyn Fn(&str)) -> Result<(), String> {
        let ndk = self.sdk_root.join("ndk").join(self.ndk_version);
        let cmake_home = self.sdk_root.join("cmake").join(CMAKE_VERSION);

        let packages = [
            (format!("ndk;{}", self.ndk_version), &ndk),
            (format!("cmake;{CMAKE_VERSION}"), &cmake_home),
        ];

        if packages.iter().any(|(_, dir)| !dir.is_dir()) {
            system::installing(|| {
                let catalog = SdkCatalog::load()?;

                for (package, dir) in packages.iter().filter(|(_, dir)| !dir.is_dir()) {
                    catalog.install(self.sdk_root, package, dir, self.accept_license)?;
                }

                return Ok(());
            })?;
        }

        let cmake = cmake_home
            .join("bin")
            .join(format!("cmake{}", std::env::consts::EXE_SUFFIX));

        let ninja = cmake_home
            .join("bin")
            .join(format!("ninja{}", std::env::consts::EXE_SUFFIX));

        let source_dir = self
            .cmake_lists
            .parent()
            .ok_or_else(|| format!("{} has no directory", self.cmake_lists.display()))?;

        let libraries = Self::libraries_dir(&self.work_dir);
        system::remove_dir(&libraries)?;

        for abi in ABIS {
            log(&format!(
                "Building native libraries for {abi} (CMake, NDK {})",
                self.ndk_version
            ));
            let build_dir = self.work_dir.join("build").join(abi);
            let mut configure = Command::new(&cmake);
            configure
                .arg("-S")
                .arg(source_dir)
                .arg("-B")
                .arg(&build_dir)
                .args(["-G", "Ninja"])
                .arg(format!("-DCMAKE_MAKE_PROGRAM={}", ninja.display()))
                .arg(format!(
                    "-DCMAKE_TOOLCHAIN_FILE={}",
                    ndk.join("build/cmake/android.toolchain.cmake").display()
                ))
                .arg(format!("-DANDROID_NDK={}", ndk.display()))
                .arg(format!("-DANDROID_ABI={abi}"))
                .arg(format!("-DANDROID_PLATFORM=android-{}", self.min_sdk))
                .arg("-DCMAKE_BUILD_TYPE=Debug")
                .arg(format!(
                    "-DCMAKE_LIBRARY_OUTPUT_DIRECTORY={}",
                    libraries.join(abi).display()
                ));
            system::run_command(&mut configure, &format!("cmake for {abi}"))?;

            let mut build = Command::new(&cmake);
            build.arg("--build").arg(&build_dir);
            system::run_command(&mut build, &format!("native build for {abi}"))?;
        }

        return Ok(());
    }
}

/// aapt2 compile of a res directory into a zip, renamed once complete: compiled resources of aars are cached
/// aapt2 compile of a res directory into <name>-<hash of its files>.zip in
/// `res_cache`, again only when they change: an app compiles those of all its
/// library modules, projects with many of them would compile all on every build
fn compiled_resources(
    sdk: &AndroidSdk,
    res_dir: &Path,
    res_cache: &Path,
    name: &str,
) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    (
        &sdk.build_tools,
        up_to_date::files_hash(&[res_dir.to_path_buf()])?,
    )
        .hash(&mut hasher);
    let output = res_cache.join(format!("{name}-{:016x}.zip", hasher.finish()));

    if output.is_file() {
        return Ok(output);
    }

    // what the directory compiled to before
    let earlier = |file: &PathBuf| {
        file.file_name()
            .and_then(|file_name| {
                file_name
                    .to_str()?
                    .strip_prefix(&format!("{name}-"))?
                    .strip_suffix(".zip")
                    .map(str::to_string)
            })
            .is_some_and(|hash| {
                hash.len() == 16 && hash.chars().all(|char| char.is_ascii_hexdigit())
            })
    };

    for file in system::files_with_extension(&[res_cache.to_path_buf()], "zip")
        .iter()
        .filter(|file| earlier(file))
    {
        fs::remove_file(file)
            .map_err(|error| format!("could not delete {}: {error}", file.display()))?;
    }

    compile_resources(&sdk.aapt2, res_dir, &output)?;

    return Ok(output);
}

fn compile_resources(aapt2: &Path, res_dir: &Path, output: &Path) -> Result<(), String> {
    system::create_parent_dir(output)?;
    let partial = system::partial_path(output);
    let mut command = Command::new(aapt2);
    command
        .arg("compile")
        .arg("--dir")
        .arg(res_dir)
        .arg("-o")
        .arg(&partial);
    system::run_command(
        &mut command,
        &format!("aapt2 compile of {}", res_dir.display()),
    )?;

    return fs::rename(&partial, output)
        .map_err(|error| format!("could not save {}: {error}", output.display()));
}

fn is_aar(file: &Path) -> bool {
    return file.extension().is_some_and(|extension| extension == "aar");
}

/// `name` (jniLibs) of the main and the debug source set where it has files, lowest priority first
fn source_set_dirs_with_files(inputs: &ModuleBuildInputs, name: &str) -> Vec<PathBuf> {
    return ["src/main", "src/debug"]
        .iter()
        .map(|source_set| inputs.directory.join(source_set).join(name))
        .filter(|dir| system::contains_files(dir))
        .collect();
}

fn cmake_dir(inputs: &ModuleBuildInputs) -> PathBuf {
    return inputs.directory.join("build/intermediates/cmake");
}

/// d8 output worth showing, and how many lines of known noise it hid. d8
/// warns about every class with Kotlin metadata newer than it knows, which is
/// harmless for dexing.
fn split_d8_noise(text: &str) -> (Vec<&str>, usize) {
    // a diagnostic starts with Warning/Info/Error and ends at the next one or an empty line
    let mut blocks: Vec<Vec<&str>> = vec![vec![]];

    for line in text.lines() {
        if line.trim().is_empty() {
            blocks.push(vec![]);
            continue;
        }

        if line.starts_with("Warning") || line.starts_with("Info") || line.starts_with("Error") {
            blocks.push(vec![]);
        }

        blocks.last_mut().unwrap().push(line);
    }

    let mut shown = vec![];
    let mut hidden = 0;

    for block in blocks.into_iter().filter(|block| !block.is_empty()) {
        let noise = block[0].starts_with("Info")
            || block
                .iter()
                .any(|line| line.contains("otlin metadata") || line.contains("kotlin.Metadata"));

        if noise {
            hidden += block.len();
        } else {
            shown.extend(block);
        }
    }

    return (shown, hidden);
}

/// Like ~/.android/debug.keystore, created on first use
fn debug_keystore(jdk: &Jdk) -> Result<PathBuf, String> {
    let keystore = system::data_dir("android").join("debug.keystore");

    if keystore.is_file() {
        return Ok(keystore);
    }

    system::create_parent_dir(&keystore)?;
    let mut command = Command::new(jdk.tool_path("keytool"));
    command
        .arg("-genkeypair")
        .arg("-keystore")
        .arg(&keystore)
        .args([
            "-storepass",
            "android",
            "-keypass",
            "android",
            "-alias",
            "androiddebugkey",
        ])
        .args(["-keyalg", "RSA", "-keysize", "2048", "-validity", "10000"])
        .args(["-dname", "CN=Android Debug,O=Android,C=US"]);
    system::run_command(&mut command, "keytool")?;

    return Ok(keystore);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d8_noise_is_hidden() {
        let output = "Warning: An error occurred when parsing kotlin metadata. This normally happens ...
Warning: Unexpected error during rewriting of Kotlin metadata for class 'kotlin.ranges.UIntProgression':
com.android.tools.r8.internal.wc4: Should never be called

Info in /x/coroutines.jar:kotlinx/coroutines/A.class:
Methods with invalid locals information:
  void a()
Warning in /x/leakcanary.jar:leakcanary/B.class:
Type `androidx.work.Worker` was not found, it is required for default or static interface methods desugaring
";

        let (shown, hidden) = split_d8_noise(output);
        assert_eq!(
            shown,
            [
                "Warning in /x/leakcanary.jar:leakcanary/B.class:",
                "Type `androidx.work.Worker` was not found, it is required for default or static interface methods desugaring",
            ]
        );
        assert_eq!(hidden, 6);
    }
}
