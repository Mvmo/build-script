import greeting.Greeting

fun main() {
    val greeting = Greeting(name = "Wire", times = 2)
    val decoded = Greeting.ADAPTER.decode(greeting.encode())
    println("Hello, ${decoded.name} x${decoded.times} (${greeting.encode().size} bytes)")
}
