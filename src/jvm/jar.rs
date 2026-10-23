// build_kotlin and build_java: modules for the JVM, a jar with its libraries next to it, and their unit tests
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::common::system;
use crate::language::runtime::{
    Arguments, BuildScriptContext, BuiltModule, FunctionParameters, FunctionSignature, Type, Value,
    builtin,
};

use super::JvmLaunch;
use super::maven::TargetPlatform;
use super::module::{
    Compilation, ModuleBuildInputs, Sources, classes_kind, create_jar, dependencies_type,
    kotlin_and_java_sources, module_params,
};
use super::testing::{UnitTests, unit_test_params};

/// `use kotlin`
pub fn kotlin_builtins() -> Vec<(&'static str, Value)> {
    return vec![(
        "build_kotlin",
        builtin(
            "build_kotlin",
            jar_params(Sources::KotlinAndJava),
            |context, args| build_module(context, args, Sources::KotlinAndJava),
        ),
    )];
}

/// `use java`: like the java plugin of Gradle, javac without Kotlin's compiler and standard library
pub fn java_builtins() -> Vec<(&'static str, Value)> {
    return vec![(
        "build_java",
        builtin("build_java", jar_params(Sources::Java), |context, args| {
            build_module(context, args, Sources::Java)
        }),
    )];
}

fn jar_params(sources: Sources) -> FunctionSignature {
    return unit_test_params(
        module_params(
            FunctionParameters::new()
                .required("name", Type::String)
                .required("dependencies", dependencies_type())
                .required("repositories", Type::list(Type::String))
                .optional("main_class", Type::String),
            &["main"],
            sources,
        ),
        sources,
    )
    .returns(Type::Module);
}

/// build/libs/<name>.jar, its runtime libraries in build/libs/lib
fn build_module(
    context: &BuildScriptContext,
    args: &Arguments,
    sources: Sources,
) -> Result<Value, String> {
    let inputs = ModuleBuildInputs::read(context, args, sources)?;

    for module in BuiltModule::transitive(&inputs.dependencies.modules) {
        if module.jvm().android.is_some() || module.jvm().classes.is_none() {
            return Err(format!(
                "{} is an Android module, build modules depending on it with build_android",
                module.name
            ));
        }
    }

    let main_class = args.get::<Option<String>>("main_class")?;
    let jar = build_jar(&inputs, args, main_class.as_deref())?;

    let unit_tests = UnitTests {
        inputs: &inputs,
        args,
        classes: vec![jar.clone()],
        platform: TargetPlatform::Jvm,
        android_jar: None,
    };

    let tests = context
        .tests
        .includes(&inputs.name)
        .then(|| unit_tests.prepare())
        .into_iter()
        .collect();

    // only jars with a Main-Class can be started
    let launch = main_class.map(|_| JvmLaunch::Jar {
        java: inputs.jdk.tool("java"),
    });

    return Ok(Value::from(inputs.built_module(
        Some(jar.clone()),
        None,
        jar,
        launch,
        tests,
    )));
}

fn build_jar(
    inputs: &ModuleBuildInputs,
    args: &Arguments,
    main_class: Option<&str>,
) -> Result<PathBuf, String> {
    let build_dir = inputs.directory.join("build");
    let resources_dir = build_dir.join("resources/main");

    let classes_dir = build_dir
        .join("classes")
        .join(classes_kind(inputs.sources))
        .join("main");

    let libs_dir = build_dir.join("libs");
    let jar = libs_dir.join(format!("{}.jar", inputs.name));

    let (kotlin_sources, java_sources) = kotlin_and_java_sources(&inputs.source_dirs);

    if kotlin_sources.is_empty() && java_sources.is_empty() {
        let dirs: Vec<String> = inputs
            .source_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect();

        let files = if inputs.sources == Sources::Java {
            ".java"
        } else {
            ".kt or .java"
        };

        return Err(format!("no {files} files found in {}", dirs.join(", ")));
    }

    let fingerprint = inputs.input_fingerprint(&inputs.name, args, &inputs.resources.inputs())?;

    if inputs.is_up_to_date(&fingerprint, &[&jar])? {
        return Ok(jar);
    }

    inputs.log(&format!("JDK {}", inputs.jdk.home.display()));
    let resolved = inputs.resolve(TargetPlatform::Jvm)?;

    if let Some(aar) = resolved
        .libraries
        .iter()
        .find(|file| file.extension().is_some_and(|extension| extension == "aar"))
    {
        return Err(format!(
            "{} is an Android library, use build_android",
            aar.display()
        ));
    }

    let mut classpath = resolved.libraries.clone();
    classpath.extend(
        resolved
            .modules
            .iter()
            .filter_map(|module| module.jvm().classes.clone()),
    );
    let mut compile_classpath = classpath.clone();
    compile_classpath.extend(resolved.compile_only.iter().cloned());

    let generated_resources = Compilation {
        inputs,
        tools: &resolved,
        variant: "main",
        source_dirs: &inputs.source_dirs,
        classpath: &compile_classpath,
        friends: &[],
        symbol_processors: resolved.symbol_processors.as_ref(),
        annotation_processors: &resolved.annotation_processor_path,
        target: TargetPlatform::Jvm,
        output: &classes_dir,
    }
    .run()?;

    inputs
        .resources
        .process(&generated_resources, &resources_dir)?;

    let libraries = copy_to_lib_dir(&classpath, &libs_dir.join("lib"))?;
    let manifest_file = build_dir.join("tmp/jar/MANIFEST.MF");
    system::write_file(&manifest_file, &jar_manifest(main_class, &libraries))?;
    create_jar(
        &inputs.jdk,
        &jar,
        Some(&manifest_file),
        &[&classes_dir, &resources_dir],
    )?;

    inputs.finish(&fingerprint, &jar)?;

    return Ok(jar);
}

/// Returns their file names for the Class-Path
fn copy_to_lib_dir(jars: &[PathBuf], lib_dir: &Path) -> Result<Vec<String>, String> {
    system::recreate_dir(lib_dir)?;
    let mut names: Vec<String> = vec![];

    let mut taken: HashSet<String> = HashSet::new();
    for jar in jars {
        let mut name = jar
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // the same file name from different groups
        if !taken.insert(name.clone()) {
            name = format!("{}-{name}", names.len());
            taken.insert(name.clone());
        }

        system::copy_file(jar, &lib_dir.join(&name))?;
        names.push(name);
    }

    return Ok(names);
}

fn jar_manifest(main_class: Option<&str>, libraries: &[String]) -> String {
    let mut manifest = manifest_line("Manifest-Version", "1.0");
    manifest += &manifest_line("Created-By", "build-script");

    if let Some(main_class) = main_class {
        manifest += &manifest_line("Main-Class", main_class);
    }

    if !libraries.is_empty() {
        let class_path: Vec<String> = libraries.iter().map(|name| format!("lib/{name}")).collect();
        manifest += &manifest_line("Class-Path", &class_path.join(" "));
    }

    return manifest;
}

/// Manifest lines may be at most 72 bytes long, continuation lines start with a space
fn manifest_line(key: &str, value: &str) -> String {
    let line = format!("{key}: {value}");
    let mut out = String::new();
    let mut rest = line.as_str();

    let mut limit = 72;
    while rest.len() > limit {
        let mut split = limit;

        while !rest.is_char_boundary(split) {
            split -= 1;
        }

        out.push_str(&rest[..split]);
        out.push_str("\r\n ");
        rest = &rest[split..];
        limit = 71;
    }

    out.push_str(rest);
    out.push_str("\r\n");

    return out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_lines_are_wrapped() {
        let value = "lib/some-library-1.0.0.jar ".repeat(10);
        let entry = manifest_line("Class-Path", &value);

        assert!(entry.split("\r\n").all(|line| line.len() <= 72));
        assert_eq!(
            entry.replace("\r\n ", ""),
            format!("Class-Path: {value}\r\n")
        );
    }
}
