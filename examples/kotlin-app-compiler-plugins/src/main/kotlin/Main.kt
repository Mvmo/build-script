import dev.drewhamilton.poko.Poko

// Poko generates equals, hashCode and toString
@Poko class Point(val x: Int, val y: Int)

fun main() {
    println(Point(1, 2))
    println("equal: ${Point(1, 2) == Point(1, 2)}")

    // power-assert explains a failed require
    val numbers = listOf(1, 2, 3)
    try {
        require(numbers.sum() == 7)
    } catch (error: IllegalArgumentException) {
        println(error.message)
    }
}
