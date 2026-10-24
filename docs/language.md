# The build language

A script is a list of statements. It looks like Python, but it is much smaller: values, variables, conditions and functions.
No loops, no IO, no HTTP.

```kotlin
// line comments and 
/* block comments */
use kotlin // loads build_kotlin and the functions around it

gson = "2.14.0" // variables hold strings, numbers, booleans, lists and modules
libraries = list("com.google.code.gson:gson:" + gson)

core = build_kotlin("core", list(), list(maven_central()), directory = "core")

app = build_kotlin(
    "app",
    libraries + list(core), // lists concatenate with +
    list(maven_central()),
    main_class = "AppKt", // named arguments, like in Kotlin
    directory = "app", // trailing commas are fine
)

print("built", app) // a module prints as the file it built

// conditions are values: the value of the block that runs
jdk = if env("CI", default = "false") == "true" { "17" } else { "21" }
```

> Relative paths in a script are relative to the directory of the script.

## Values

- Strings in double quotes, without escapes.
- Integers, `true` and `false`.
- Lists from `list(...)`.
- Modules, which the build functions return.
- Functions.

## Operators

- `+` adds numbers, concatenates strings and lists, and joins a string with a number.
- `-`, `*` and `/` work on numbers.
- `==` and `!=` compare, `<`, `<=`, `>` and `>=` order numbers and strings.
- `!` negates, `&&` and `||` combine booleans. Their right side only runs when the left one does not decide.

## Conditions

`if condition { ... } else if other { ... } else { ... }` is a value: the value of the block that runs, `unit` without an `else`. The condition must be a boolean. What a block defines stays in it.

## Calls

Arguments are passed by position or by name. Like in Kotlin, positional arguments may follow named ones only when those stand at their own position.
Every argument is checked against the type of its parameter, an error names the place:

```
project.build:4:5: build_kotlin: argument 'dependencies' must be list<str | module>, got str
```

The built-in functions and their parameters are written in lower case with underscores: `build_kotlin`, `maven_central`, `main_class`. A script with the old camel case names is told the new ones.

## Functions every script has

| Function                   | What it does                                                                 |
|----------------------------|------------------------------------------------------------------------------|
| `print(values...)`         | Prints the values                                                            |
| `list(items...)`           | A list                                                                       |
| `type(value)`              | The type of a value, like `list<str>`                                        |
| `env(name, default = ...)` | An environment variable, an error when it is not set and there is no default |
| `has_env(name)`            | Whether an environment variable is set                                       |
| `current_time_millis()`    | The time in milliseconds                                                     |

The functions of a language come with its `use`, see [java.md](java.md), [kotlin.md](kotlin.md) and [android.md](android.md).

## Your own functions

Functions are values like strings or lists: `fn`, the parameters with their types, the type of the result, and a block which last expression is the result.

```kotlin
greet = fn (name: str, greeting: str = "Hello") -> str {
    text = greeting + ", " + name      // variables of a call stay in the call
    text
}

greet("Ada")                           // Hello, Ada
greet(greeting = "Hi", name = "Bob")   // named arguments and defaults

// what every module of a project shares, in one place
repositories = list(google(), maven_central())

library = fn (name: str, namespace: str, dependencies: list<str | module> = list()) -> module {
    build_android_library(
        name,
        namespace,
        dependencies,
        repositories,
        min_sdk = 28,
        directory = name
    )
}

core = library("core", "com.example.core")
```

- Types are `str`, `int`, `bool`, `module`, `any`, `unit`, `list<str>`, unions like `str | module` and function types like `fn(str) -> bool`. Errors and `type(...)` use the same names.
- Without `-> type` a function may return anything, without a value it returns `unit`.
- Arguments and results are checked against their types.
- Defaults are evaluated once, when the function is defined.
- A function sees the variables of the place where it was defined. Its assignments stay in the call.
- Functions can be passed to functions and returned from them:

  ```kotlin
  apply = fn (f: fn(str) -> str, value: str) -> str { f(value) }
  adder = fn (a: int) -> fn(int) -> int { fn (b: int) -> int { a + b } }
  adder(2)(3)                          // 5
  ```

> A function that calls itself ends with an `if`, at most 64 calls deep.
