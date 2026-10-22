// Templates of JVM modules for `build-script new` and `build-script add`: Kotlin and Java apps and libraries, Android apps and libraries
use crate::project::{ModuleTemplate, NewModule};

pub const MODULE_KINDS: &[&str] = &[
    "kotlin-app",
    "kotlin-library",
    "java-app",
    "java-library",
    "android-app",
    "android-library",
];

/// The modules each kind of project starts with, dependencies first: the
/// kind of module and its directory, None for a module at the project root
pub const PROJECT_KINDS: &[(&str, &[(&str, Option<&str>)])] = &[
    ("java", &[("java-app", None)]),
    ("kotlin", &[("kotlin-app", None)]),
    ("android", &[("android-app", None)]),
    (
        "multi",
        &[
            ("kotlin-library", Some("core")),
            ("kotlin-app", Some("app")),
        ],
    ),
];

/// Java and Kotlin keywords, which cannot be part of a package name
const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "fun",
    "goto",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "int",
    "interface",
    "is",
    "long",
    "native",
    "new",
    "null",
    "object",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "void",
    "volatile",
    "when",
    "while",
];

const MAVEN_CENTRAL: &[&str] = &["maven_central()"];
const GOOGLE_AND_MAVEN_CENTRAL: &[&str] = &["google()", "maven_central()"];

/// Like the Java naming conventions name packages after a domain: reversed,
/// characters other than letters and digits become underscores, a keyword gets
/// one appended and a label that starts with a digit one prefixed: mvmo.dev -> dev.mvmo
pub fn package_of_domain(domain: &str) -> String {
    let segments: Vec<String> = domain
        .split('.')
        .rev()
        .map(|label| {
            let segment = package_segment(label);
            if segment.starts_with(|char: char| char.is_ascii_digit()) {
                format!("_{segment}")
            } else {
                segment
            }
        })
        .collect();
    return segments.join(".");
}

/// my-App -> my_app, a keyword gets an underscore
pub fn package_segment(name: &str) -> String {
    let segment: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char
            } else {
                '_'
            }
        })
        .collect();

    if KEYWORDS.contains(&segment.as_str()) {
        return format!("{segment}_");
    }

    return segment;
}

/// Java identifiers separated by dots, none of them a keyword
pub fn checked_package(package: &str) -> Result<String, String> {
    let valid = package.split('.').all(|segment| {
        segment
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && segment
                .chars()
                .all(|char| char.is_ascii_alphanumeric() || char == '_')
            && !KEYWORDS.contains(&segment)
    });

    if !valid {
        return Err(format!(
            "'{package}' is not a package name: identifiers separated by dots, like dev.mvmo"
        ));
    }

    return Ok(package.to_string());
}

pub fn module_template(kind: &str, module: &NewModule) -> Option<ModuleTemplate> {
    let package_dir = module.package.replace('.', "/");
    let package = &module.package;
    let name = &module.name;

    let dependencies = format!(
        "list({})",
        module
            .dependencies
            .iter()
            .map(|(variable, _)| variable.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let template = match kind {
        "kotlin-app" => {
            let main = match module.dependencies.first() {
                Some((_, library_package)) => format!(
                    "package {package}\n\nimport {library_package}.greeting\n\nfun main() {{\n    println(greeting(\"{name}\"))\n}}\n"
                ),
                None => format!(
                    "package {package}\n\nfun main() {{\n    println(\"Hello from {name}\")\n}}\n"
                ),
            };
            ModuleTemplate {
                platform: "kotlin",
                repositories: MAVEN_CENTRAL,
                is_app: true,
                files: vec![(format!("src/main/kotlin/{package_dir}/Main.kt"), main)],
                function: "build_kotlin",
                positional: vec![quoted(name), dependencies],
                named: vec![("main_class", quoted(&format!("{package}.MainKt")))],
            }
        }
        "kotlin-library" => ModuleTemplate {
            platform: "kotlin",
            repositories: MAVEN_CENTRAL,
            is_app: false,
            files: vec![(
                format!("src/main/kotlin/{package_dir}/Greeting.kt"),
                format!(
                    "package {package}\n\nfun greeting(name: String): String = \"Hello, $name, from {name}\"\n"
                ),
            )],
            function: "build_kotlin",
            positional: vec![quoted(name), dependencies],
            named: vec![],
        },
        "java-app" => ModuleTemplate {
            platform: "java",
            repositories: MAVEN_CENTRAL,
            is_app: true,
            files: vec![(
                format!("src/main/java/{package_dir}/Main.java"),
                format!(
                    "package {package};\n\npublic class Main {{\n    public static void main(String[] args) {{\n        System.out.println(\"Hello from {name}\");\n    }}\n}}\n"
                ),
            )],
            function: "build_java",
            positional: vec![quoted(name), dependencies],
            named: vec![("main_class", quoted(&format!("{package}.Main")))],
        },
        "java-library" => ModuleTemplate {
            platform: "java",
            repositories: MAVEN_CENTRAL,
            is_app: false,
            files: vec![(
                format!("src/main/java/{package_dir}/Greeting.java"),
                format!(
                    "package {package};\n\npublic final class Greeting {{\n    private Greeting() {{\n    }}\n\n    public static String greeting(String name) {{\n        return \"Hello, \" + name + \", from {name}\";\n    }}\n}}\n",
                ),
            )],
            function: "build_java",
            positional: vec![quoted(name), dependencies],
            named: vec![],
        },
        "android-app" => ModuleTemplate {
            platform: "android",
            repositories: GOOGLE_AND_MAVEN_CENTRAL,
            is_app: true,
            files: vec![
                (
                    "src/main/AndroidManifest.xml".to_string(),
                    APP_MANIFEST.to_string(),
                ),
                (
                    "src/main/res/values/strings.xml".to_string(),
                    format!(
                        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n    <string name=\"app_name\">{name}</string>\n    <string name=\"greeting\">Hello from {name}</string>\n</resources>\n",
                    ),
                ),
                (
                    "src/main/res/layout/activity_main.xml".to_string(),
                    MAIN_LAYOUT.to_string(),
                ),
                (
                    format!("src/main/kotlin/{package_dir}/MainActivity.kt"),
                    format!(
                        "package {package}\n\nimport android.app.Activity\nimport android.os.Bundle\n\nclass MainActivity : Activity() {{\n    override fun onCreate(savedInstanceState: Bundle?) {{\n        super.onCreate(savedInstanceState)\n        setContentView(R.layout.activity_main)\n    }}\n}}\n",
                    ),
                ),
            ],
            function: "build_android",
            positional: vec![quoted(name), quoted(package), dependencies],
            named: vec![],
        },
        "android-library" => ModuleTemplate {
            platform: "android",
            repositories: GOOGLE_AND_MAVEN_CENTRAL,
            is_app: false,
            files: vec![(
                format!("src/main/kotlin/{package_dir}/Greeting.kt"),
                format!(
                    "package {package}\n\nfun greeting(name: String): String = \"Hello, $name, from {name}\"\n"
                ),
            )],
            function: "build_android_library",
            positional: vec![quoted(name), quoted(package), dependencies],
            named: vec![],
        },
        _ => return None,
    };

    return Some(template);
}

const APP_MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <application android:label="@string/app_name" android:theme="@android:style/Theme.Material.Light.DarkActionBar">
        <activity android:name=".MainActivity" android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
"#;

const MAIN_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<FrameLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:layout_width="wrap_content"
        android:layout_height="wrap_content"
        android:layout_gravity="center"
        android:text="@string/greeting"
        android:textSize="24sp" />
</FrameLayout>
"#;

fn quoted(value: &str) -> String {
    return format!("\"{value}\"");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_become_packages_like_the_java_naming_conventions_say() {
        assert_eq!(package_of_domain("mvmo.dev"), "dev.mvmo");
        assert_eq!(package_of_domain("example.com"), "com.example");
        // hyphens, keywords and digits at the start do not make identifiers
        assert_eq!(
            package_of_domain("my-site.example.org"),
            "org.example.my_site"
        );
        assert_eq!(
            package_of_domain("int.1password.com"),
            "com._1password.int_"
        );

        assert_eq!(package_segment("My-App"), "my_app");
        assert_eq!(package_segment("native"), "native_");
        assert_eq!(checked_package("dev.mvmo"), Ok("dev.mvmo".to_string()));
        assert!(checked_package("dev..mvmo").is_err());
        assert!(checked_package("dev.mvmo.new").is_err());
        assert!(checked_package("1dev").is_err());
    }
}
