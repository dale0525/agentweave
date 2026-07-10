plugins {
  alias(libs.plugins.android.application)
  alias(libs.plugins.kotlin.compose)
}

android {
  namespace = "com.generalagent.mobile"
  compileSdk = 37

  defaultConfig {
    applicationId = "com.generalagent.mobile"
    minSdk = 31
    targetSdk = 36
    versionCode = 1
    versionName = "0.1.0"
    testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
  }

  buildFeatures {
    compose = true
  }

  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
  }

  testOptions {
    unitTests.isIncludeAndroidResources = true
  }

  sourceSets.named("main") {
    jniLibs.directories.add("build/generated/rustJniLibs")
  }
}

dependencies {
  val composeBom = platform(libs.androidx.compose.bom)
  implementation(composeBom)
  implementation(libs.androidx.activity.compose)
  implementation(libs.androidx.core.ktx)
  implementation(libs.androidx.lifecycle.runtime.ktx)
  implementation(libs.androidx.compose.material3)
  implementation(libs.androidx.compose.material.icons.extended)
  implementation(libs.androidx.compose.ui)
  implementation(libs.androidx.compose.ui.tooling.preview)
  debugImplementation(libs.androidx.compose.ui.tooling)

  testImplementation(libs.junit)
  testImplementation(libs.robolectric)
}

val buildRustNative by tasks.registering(Exec::class) {
  workingDir(rootProject.projectDir.resolve("../.."))
  commandLine("node", "scripts/build-android-rust.mjs")
}

tasks.named("preBuild") {
  dependsOn(buildRustNative)
}
