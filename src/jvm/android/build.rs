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
use crate::language::runtime::{Args, BuildScriptContext, BuiltModule};

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

/// Like with the Gradle plugin, view bindings come with their runtime, which
/// consumers of a library need as well
pub fn read_android_inputs(
    context: &BuildScriptContext,
    args: &Args,
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
    args: &Args,
    name: &str,
) -> Result<Vec<PathBuf>, String> {
    let dirs: Vec<String> = args.get(name)?;
    return Ok(dirs
        .iter()
        .map(|dir| inputs.directory.join(dir))
        .filter(|dir| system::contains_files(dir))
        .collect());
}

/// What the outputs of an Android module depend on besides the sources:
/// resources, AIDL, manifests, assets, native libraries, Java resources
pub fn android_input_paths(
    inputs: &ModuleBuildInputs,
    args: &Args,
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

impl AndroidBuild {
    pub fn prepare(inputs: ModuleBuildInputs, args: &Args) -> Result<Self, String> {
        let build_dir = inputs.directory.join("build");
        return Self::prepare_variant(inputs, args, "debug", build_dir);
    }

    /// The module with its instrumented tests, built apart from the module: the
    /// resources and assets of src/androidTest lie over those of the module
    pub fn prepare_android_tests(inputs: ModuleBuildInputs, args: &Args) -> Result<Self, String> {
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
        args: &Args,
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
        inputs.log(&format!(
            "Android SDK {} (android-{compile_sdk}, build-tools {build_tools})",
            sdk.root.display()
        ));
        inputs.log(&format!("JDK {}", inputs.jdk.home.display()));

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

    /// aapt2 link of the app: its resources and those of all libraries, the
    /// ids of all of them in `symbols`
    pub fn link(
        &self,
        manifest: &Path,
        own: &[PathBuf],
        output: &Path,
        symbols: &Path,
        extra: &[String],
    ) -> Result<(), String> {
        return self.run_link(manifest, own, true, output, symbols, false, extra);
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
        self.run_link(
            manifest,
            own,
            true,
            &work_dir.join("library.ap_"),
            &with_libraries,
            true,
            &[],
        )?;

        if !self.non_transitive_r {
            return system::copy_file(&with_libraries, symbols);
        }

        let own_only = work_dir.join("R-own.txt");
        self.run_link(
            manifest,
            own,
            false,
            &work_dir.join("library-own.ap_"),
            &own_only,
            true,
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

    fn run_link(
        &self,
        manifest: &Path,
        own: &[PathBuf],
        with_libraries: bool,
        output: &Path,
        symbols: &Path,
        library: bool,
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

        if library {
            command.arg("--non-final-ids");
            // aapt2 links stubs of the framework itself only as an app
            if self.namespace != "android" {
                command.args(["--static-lib", "--merge-only"]);
            }
        }

        // later overlays win: libraries, then the module itself
        let libraries = if with_libraries {
            &self.libraries[..]
        } else {
            &[][..]
        };

        for compiled in libraries
            .iter()
            .flat_map(|library| &library.compiled)
            .chain(own)
        {
            command.arg("-R").arg(compiled);
        }

        return system::run(&mut command, "aapt2 link");
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

        self.inputs
            .log(&format!("Generating Java for {} AIDL file(s)", files.len()));
        for file in &files {
            let mut command = Command::new(&self.sdk.aidl);
            command.arg(format!("-p{}", self.sdk.framework_aidl.display()));

            for include in self.aidl_dirs.iter().chain(&self.aidl_imports) {
                command.arg(format!("-I{}", include.display()));
            }

            command.arg(format!("-o{}", output.display())).arg(file);
            system::run(&mut command, &format!("aidl {}", file.display()))?;
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
                    &[system::extracted_in_cache(file)?],
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

        let target = system::extracted_in_cache(aar)?;

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
    let partial = PathBuf::from(format!("{}.part", output.display()));
    let mut command = Command::new(aapt2);
    command
        .arg("compile")
        .arg("--dir")
        .arg(res_dir)
        .arg("-o")
        .arg(&partial);
    system::run(
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
