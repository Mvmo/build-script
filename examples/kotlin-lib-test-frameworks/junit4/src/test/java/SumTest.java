import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class SumTest {
    @Test
    public void addsTwoNumbers() {
        assertEquals(3, SumKt.sum(1, 2));
    }
}
