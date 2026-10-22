package com.example.greeting

import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.widget.TextView

object Greetings {
    fun text(context: Context): String = context.getString(R.string.greeting_text)

    val debug: Boolean get() = BuildConfig.DEBUG
}

class GreetingActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = Greetings.text(this@GreetingActivity) })
    }
}
