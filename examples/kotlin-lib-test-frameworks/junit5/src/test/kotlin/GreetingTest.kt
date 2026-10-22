import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class GreetingTest {
    @Test
    fun `greets by name`() {
        assertEquals("Hello, JUnit 5", greeting("JUnit 5"))
    }
}
