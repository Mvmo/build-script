import kotlin.test.assertEquals
import org.spekframework.spek2.Spek
import org.spekframework.spek2.style.specification.describe

object GreetingSpec : Spek({
    describe("greeting") {
        it("greets by name") {
            assertEquals("Hello, Spek 2", greeting("Spek 2"))
        }
    }
})
