package com.example.views

import android.os.Bundle
import androidx.appcompat.app.AppCompatActivity
import com.example.views.databinding.ActivityMainBinding

class MainActivity : AppCompatActivity() {
    // Kotlin calling Java
    private val counter = Counter()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        binding.header.title.setText(R.string.title)
        binding.messageText.text = "API: ${BuildConfig.API_URL}"
        binding.countButton.text = "Click me"
        binding.countButton.setOnClickListener { binding.countButton.text = counter.increment() }
    }
}
