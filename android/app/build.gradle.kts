plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}
android {
    namespace = "io.github.kokorodonguri.riscord"
    compileSdk = 35
    defaultConfig {
        applicationId = "io.github.kokorodonguri.riscord"
        minSdk = 29
        targetSdk = 35
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        versionCode = 400
        versionName = "0.4.0"
        ndk { abiFilters += "arm64-v8a" }
    }
    // release の署名鍵は環境変数から読む（CI では secrets）。無い環境では未署名の release しか作れない
    val releaseKeystore = System.getenv("RISCORD_ANDROID_KEYSTORE")
    signingConfigs {
        if (!releaseKeystore.isNullOrEmpty()) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("RISCORD_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("RISCORD_ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("RISCORD_ANDROID_KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        // 利用者に配るのは release。debuggable にしないので、adb からアプリのデータ（トークン）を読まれない
        release {
            isDebuggable = false
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
        }
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}
dependencies {
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation(platform("androidx.compose:compose-bom:2025.04.01"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.0")
    implementation("androidx.lifecycle:lifecycle-process:2.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("io.coil-kt:coil-compose:2.7.0")

    implementation(platform("androidx.compose:compose-bom:2025.04.01"))
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
}

// Rust のコア（libriscord_core.so）を作る。入力が変わっていなければ up-to-date で飛ばす。
// 既定は cargo の release（debug だと音声の処理が重い）。-Priscord.nativeProfile=debug で debug にできる
val repoRoot = rootProject.projectDir.parentFile
val nativeProfile = providers.gradleProperty("riscord.nativeProfile").orElse("release")
val buildRustNative by tasks.registering(Exec::class) {
    onlyIf { !providers.gradleProperty("riscord.usePrebuiltNative").isPresent }
    workingDir(repoRoot)
    commandLine("bash", "scripts/build-android-native.sh", nativeProfile.get())
    inputs.property("profile", nativeProfile)
    inputs.property("ndk", providers.environmentVariable("ANDROID_NDK_HOME").orElse(""))
    // riscord-core は src/ のモジュールを #[path] で取り込んでいる
    inputs.files(
        File(repoRoot, "Cargo.toml"), File(repoRoot, "Cargo.lock"),
        File(repoRoot, "crates/riscord-core/Cargo.toml"), File(repoRoot, "scripts/build-android-native.sh"),
    ).withPathSensitivity(PathSensitivity.RELATIVE)
    inputs.dir(File(repoRoot, "crates/riscord-core/src")).withPathSensitivity(PathSensitivity.RELATIVE)
    inputs.dir(File(repoRoot, "src")).withPathSensitivity(PathSensitivity.RELATIVE)
    outputs.file(File(projectDir, "src/main/jniLibs/arm64-v8a/libriscord_core.so"))
}
tasks.named("preBuild") { dependsOn(buildRustNative) }
