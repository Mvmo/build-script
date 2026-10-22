package com.example.views;

/** Java calling Kotlin: the label comes from Labels.kt */
public final class Counter {
    private int count;

    public String increment() {
        count++;
        return Labels.INSTANCE.clicks(count);
    }
}
