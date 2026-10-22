import io.kotest.core.spec.style.StringSpec
import io.kotest.matchers.shouldBe

class GreetingSpec : StringSpec({
    "greets by name" {
        greeting("Kotest") shouldBe "Hello, Kotest"
    }
})
