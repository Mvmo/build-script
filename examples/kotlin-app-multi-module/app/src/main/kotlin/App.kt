import core.Greeter

fun main(args: Array<String>) {
    val name = args.joinToString(" ").ifEmpty { "multi-module" }
    println(Greeter.greet(name))
}
