mod lexer;
mod parser;

fn main() {
    let source = r#"
        use kotlin
        use "otherscript"

        x = "hello"
        y = x == "hello"
        print(greet(x), !y)
        z = -a + b * (c - d)
        a = b = c
    "#;

    match parser::Parser::new(source).parse_script() {
        Ok(script) => {
            for statement in script.statements {
                println!("{:#?}", statement.variant);
            }
        },
        Err(error) => println!("{:?}", error),
    }

    for err in ["f() = x", "x = (a + b"] {
        println!("{err} -> {:?}", parser::Parser::new(err).parse_script().err());
    }
}
