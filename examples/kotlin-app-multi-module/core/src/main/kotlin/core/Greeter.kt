package core

import com.google.gson.Gson

data class Greeting(val text: String, val from: String)

object Greeter {
    fun greet(name: String): String = Gson().toJson(Greeting("Hello, $name", from = "core"))
}
