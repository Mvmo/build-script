import org.testng.Assert.assertEquals
import org.testng.annotations.Test

class GreetingTest {
    @Test
    fun greetsByName() {
        assertEquals(greeting("TestNG"), "Hello, TestNG")
    }
}
