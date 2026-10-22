package com.example.app

import android.os.Bundle
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import com.example.greeting.Greetings
import com.example.shared.shout

class MainActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        val version = "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})"
        findViewById<TextView>(R.id.message).text = "${shout(Greetings.text(this))}\n$version"
    }
}
