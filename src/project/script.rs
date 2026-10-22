// Adding a module to a build script: its `use`, its repositories and its build call, placed so apps come last.
// The script is parsed to find the places, everything else stays as it is written.
use crate::language::parser::{Expression, ExpressionType, Parser, StatementType, UseTarget};

use super::{ModuleTemplate, NewModule};

/// The script with the module added as `variable`. Libraries go before the
/// first app, so apps can depend on them, apps go to the end.
pub fn with_module(
    source: &str,
    variable: &str,
    module: &NewModule,
    template: &ModuleTemplate,
) -> Result<String, String> {
    let script = Parser::new(source).parse_script().map_err(|error| {
        format!(
            "line {}: {}",
            line_of(source, error.span.start),
            error.variant
        )
    })?;

    let mut uses: Vec<(&str, usize)> = vec![];
    let mut variables: Vec<&str> = vec![];
    let mut module_names: Vec<&str> = vec![];
    let mut repositories: Option<(&Expression, usize)> = None;
    let mut first_app: Option<usize> = None;

    for statement in &script.statements {
        let expression = match &statement.variant {
            StatementType::Use(UseTarget::Ident(platform)) => {
                uses.push((platform.str.as_str(), statement.span.end));
                continue;
            }
            StatementType::Use(_) => continue,
            StatementType::Expression(expression) => expression,
        };

        let value = match &expression.variant {
            ExpressionType::Assignment { target, value } => {
                let name = identifier(target).unwrap_or_default();
                variables.push(name);
                if name == "repositories" {
                    repositories = Some((value, statement.span.start));
                }
                value.as_ref()
            }
            _ => expression,
        };

        if let ExpressionType::FunctionCall { target, args } = &value.variant {
            if let Some(ExpressionType::String(name)) = args.first().map(|arg| &arg.variant) {
                module_names.push(name);
            }

            let is_app = identifier(target) == Some("build_android")
                || args
                    .iter()
                    .any(|arg| argument_name(arg) == Some("main_class"));

            if is_app && first_app.is_none() {
                first_app = Some(statement.span.start);
            }
        }
    }

    if module_names.contains(&module.name.as_str()) {
        return Err(format!(
            "a module named {} is already in the script",
            module.name
        ));
    }

    if variables.contains(&variable) {
        return Err(format!("the script already has a variable {variable}"));
    }

    let module_position = match first_app {
        Some(app) if !template.is_app => Some(start_of_line_with_comments(source, app)),
        _ => None,
    };

    // inserted at their offset, at equal offsets in the order they were added
    let mut edits: Vec<(usize, String)> = vec![];
    let mut repositories_argument = "repositories".to_string();
    let mut new_repositories = None;

    match repositories {
        Some((value, start)) if module_position.is_none_or(|position| start < position) => {
            match list_arguments(source, value) {
                Some((after_paren, arguments)) => {
                    let missing: Vec<&str> = template
                        .repositories
                        .iter()
                        .filter(|repository| {
                            !arguments
                                .iter()
                                .any(|argument| calls(argument, repository.trim_end_matches("()")))
                        })
                        .copied()
                        .collect();

                    if !missing.is_empty() {
                        let separator = if arguments.is_empty() { "" } else { ", " };
                        edits.push((
                            after_paren,
                            format!("{}{separator}", missing.join(", "))
                        ));
                    }
                }
                // no list(...) to extend: the module names its repositories itself
                None => {
                    repositories_argument = format!(
                        "list({})",
                        template.repositories.join(", ")
                    )
                }
            }
        }
        Some(_) => repositories_argument = format!("list({})", template.repositories.join(", ")),
        None => {
            new_repositories = Some(format!(
                "repositories = list({})",
                template.repositories.join(", ")
            ))
        }
    }

    let needs_use = !uses
        .iter()
        .any(|(platform, _)| *platform == template.platform);

    let has_statements = !script.statements.is_empty();

    match uses.last() {
        // below the last use
        Some((_, end)) => {
            let mut text = String::new();

            if needs_use {
                text.push_str(&format!("\nuse {}", template.platform));
            }

            if let Some(repositories) = &new_repositories {
                text.push_str(&format!("\n\n{repositories}"));
            }

            edits.push((*end, text));
        }
        None if has_statements => {
            let mut text = format!("use {}\n", template.platform);

            if let Some(repositories) = &new_repositories {
                text.push_str(&format!("\n{repositories}\n"));
            }

            text.push('\n');
            edits.push((start_of_line(source, script.statements[0].span.start), text));
        }
        None => {
            let mut text = if source.is_empty() || source.ends_with('\n') {
                String::new()
            } else {
                "\n".to_string()
            };

            text.push_str(&format!("use {}\n", template.platform));

            if let Some(repositories) = &new_repositories {
                text.push_str(&format!("\n{repositories}\n"));
            }

            edits.push((source.len(), text));
        }
    }

    let call = build_call(variable, module, template, &repositories_argument);
    match module_position {
        Some(position) => edits.push((position, format!("{call}\n\n"))),
        None if !has_statements || source.ends_with('\n') => {
            edits.push((source.len(), format!("\n{call}\n")))
        }
        None => edits.push((source.len(), format!("\n\n{call}\n"))),
    }

    return Ok(apply(source, edits));
}

/// `variable = function(positional..., repositories, named = value..., directory = "...")`
fn build_call(
    variable: &str,
    module: &NewModule,
    template: &ModuleTemplate,
    repositories: &str,
) -> String {
    let mut lines = vec![format!("{variable} = {}(", template.function)];
    lines.extend(
        template
            .positional
            .iter()
            .map(|argument| format!("    {argument},")),
    );
    lines.push(format!("    {repositories},"));
    lines.extend(
        template
            .named
            .iter()
            .map(|(name, value)| format!("    {name} = {value},")),
    );
    if let Some(directory) = &module.directory {
        lines.push(format!("    directory = \"{directory}\","));
    }
    lines.push(")".to_string());
    return lines.join("\n");
}

fn apply(source: &str, mut edits: Vec<(usize, String)>) -> String {
    let mut result = source.to_string();

    // from the end, so earlier offsets stay valid; at one offset the last added goes in first
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse((edits[*index].0, *index)));
    for index in order {
        let (offset, text) = std::mem::take(&mut edits[index]);
        result.insert_str(offset, &text);
    }

    return result;
}

/// The arguments of a `list(...)` call and the offset right after its parenthesis
fn list_arguments<'a>(source: &str, value: &'a Expression) -> Option<(usize, &'a [Expression])> {
    let ExpressionType::FunctionCall { target, args } = &value.variant else {
        return None;
    };

    if identifier(target) != Some("list") {
        return None;
    }

    let paren = target.span.end + source[target.span.end..].find('(')?;

    return Some((paren + 1, args));
}

/// Whether the expression calls the function `name`
fn calls(expression: &Expression, name: &str) -> bool {
    let ExpressionType::FunctionCall { target, .. } = &expression.variant else {
        return false;
    };
    return identifier(target) == Some(name);
}

fn identifier(expression: &Expression) -> Option<&str> {
    let ExpressionType::Ident(name) = &expression.variant else {
        return None;
    };
    return Some(name);
}

/// `name` of a named argument `name = value`
fn argument_name(argument: &Expression) -> Option<&str> {
    let ExpressionType::Assignment { target, .. } = &argument.variant else {
        return None;
    };
    return identifier(target);
}

fn start_of_line(source: &str, offset: usize) -> usize {
    return source[..offset]
        .rfind('\n')
        .map(|newline| newline + 1)
        .unwrap_or(0);
}

/// The start of the line, or of the comment lines right above it, which belong to it
fn start_of_line_with_comments(source: &str, offset: usize) -> usize {
    let mut start = start_of_line(source, offset);
    while start > 0 {
        let previous = start_of_line(source, start - 1);
        if !source[previous..start].trim_start().starts_with("//") {
            break;
        }
        start = previous;
    }
    return start;
}

fn line_of(source: &str, offset: usize) -> usize {
    return source.get(..offset).unwrap_or(source).matches('\n').count() + 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> NewModule {
        return NewModule {
            name: name.to_string(),
            directory: Some(name.to_string()),
            package: format!("com.example.{name}"),
            dependencies: vec![],
        };
    }

    fn template(
        platform: &'static str,
        repositories: &'static [&'static str],
        is_app: bool,
    ) -> ModuleTemplate {
        return ModuleTemplate {
            platform,
            repositories,
            is_app,
            files: vec![],
            function: if platform == "android" {
                "build_android_library"
            } else {
                "build_kotlin"
            },
            positional: vec!["\"lib\"".to_string(), "list()".to_string()],
            named: vec![],
        };
    }

    #[test]
    fn libraries_go_before_the_first_app_with_its_comments() {
        let source = "use kotlin\n\nrepositories = list(maven_central())\n\n// the app\n// starts here\napp = build_kotlin(\"app\", list(), repositories, main_class = \"MainKt\")\n";
        let result = with_module(
            source,
            "lib",
            &module("lib"),
            &template("kotlin", &["maven_central()"], false),
        )
        .unwrap();
        assert_eq!(
            result,
            "use kotlin\n\nrepositories = list(maven_central())\n\nlib = build_kotlin(\n    \"lib\",\n    list(),\n    repositories,\n    directory = \"lib\",\n)\n\n// the app\n// starts here\napp = build_kotlin(\"app\", list(), repositories, main_class = \"MainKt\")\n"
        );
    }

    #[test]
    fn missing_uses_and_repositories_are_added() {
        let source = "use kotlin\nrepositories = list(maven_central()) // central\ncore = build_kotlin(\"core\", list(), repositories)";
        let result = with_module(
            source,
            "ui",
            &module("ui"),
            &template("android", &["google()", "maven_central()"], false),
        )
        .unwrap();
        assert_eq!(
            result,
            "use kotlin\nuse android\nrepositories = list(google(), maven_central()) // central\ncore = build_kotlin(\"core\", list(), repositories)\n\nui = build_android_library(\n    \"lib\",\n    list(),\n    repositories,\n    directory = \"ui\",\n)\n"
        );

        // a script without anything gets all of it
        let result = with_module(
            "",
            "lib",
            &module("lib"),
            &template("kotlin", &["maven_central()"], false),
        )
        .unwrap();
        assert_eq!(
            result,
            "use kotlin\n\nrepositories = list(maven_central())\n\nlib = build_kotlin(\n    \"lib\",\n    list(),\n    repositories,\n    directory = \"lib\",\n)\n"
        );

        // repositories that are no list(...) stay as they are, the module names its own
        let result = with_module(
            "use kotlin\nrepositories = shared\n",
            "lib",
            &module("lib"),
            &template("kotlin", &["maven_central()"], false),
        )
        .unwrap();
        assert!(result.contains("    list(maven_central()),\n"), "{result}");
    }

    #[test]
    fn names_already_in_the_script_are_refused() {
        let source = "use kotlin\ncore = build_kotlin(\"core\", list(), list(maven_central()))\n";
        let error = with_module(
            source,
            "core_lib",
            &module("core"),
            &template("kotlin", &["maven_central()"], false),
        )
        .unwrap_err();
        assert!(error.contains("module named core"), "{error}");
        let error = with_module(
            source,
            "core",
            &module("other"),
            &template("kotlin", &["maven_central()"], false),
        )
        .unwrap_err();
        assert!(error.contains("variable core"), "{error}");
        assert!(
            with_module(
                "x = list(",
                "lib",
                &module("lib"),
                &template("kotlin", &[], false)
            )
            .unwrap_err()
            .starts_with("line 1:")
        );
    }
}
