import com.google.gson.Gson
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking

data class Library(val name: String, val inPackage: String)

fun main() = runBlocking {
    val libraries = listOf(Gson::class.java, Job::class.java).map { library ->
        async { Library(library.simpleName, library.packageName) }
    }.awaitAll()

    println(Gson().toJson(libraries))
}
