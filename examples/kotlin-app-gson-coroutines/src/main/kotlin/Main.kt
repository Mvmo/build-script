import com.google.gson.GsonBuilder
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking

data class Build(val project: String, val greeting: String, val steps: List<String>)

fun main() = runBlocking {
    val greeting = object {}.javaClass.getResource("/greeting.txt")?.readText()?.trim() ?: "?"

    val steps = listOf("resolve", "compile", "package").map { step ->
        async {
            delay(10)
            "$step: ok"
        }
    }.awaitAll()

    val gson = GsonBuilder().setPrettyPrinting().create()
    println(gson.toJson(Build("kotlin-app", greeting, steps)))
}
