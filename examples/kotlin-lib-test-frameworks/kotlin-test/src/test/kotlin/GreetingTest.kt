import kotlin.test.Test
import kotlin.test.assertEquals

class GreetingTest {
    @Test
    fun greetsByName() {
        assertEquals("Hello, kotlin.test", greeting("kotlin.test"))
    }
}
