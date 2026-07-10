package com.generalagent.mobile.runtime

fun androidMvpCapabilities(): List<String> =
  listOf(
    "network.http",
    "filesystem.app_data",
    "secure_storage",
    "model.http_provider",
  )
