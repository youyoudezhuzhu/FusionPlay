import org.gradle.api.tasks.testing.Test
import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.serialization")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

val repositoryRoot = layout.projectDirectory.dir("../../..")
val generatedNativeLibs = layout.buildDirectory.dir("generated/fusionPlayJniLibs")

// Release builds default to a single 64-bit ABI: the universal APK bundled
// three copies of the Flutter engine and the Rust core (~68 MB uncompressed),
// which is unnecessary for modern phones/TVs. CI sets FUSIONPLAY_ABIS=arm64-v8a.
// Local debug builds keep every ABI unless the environment says otherwise.
val allAndroidAbis = listOf("armeabi-v7a", "arm64-v8a", "x86_64")
val requestedAndroidAbis: List<String> = System.getenv("FUSIONPLAY_ABIS")
    ?.split(',', ';', ' ')
    ?.map { it.trim() }
    ?.filter { it.isNotEmpty() && it in allAndroidAbis }
    ?.distinct()
    ?.takeIf { it.isNotEmpty() }
    ?: allAndroidAbis
val nativeAbis = requestedAndroidAbis

// Stable release signing: CI (and maintainers) provide a fixed keystore through
// environment variables so every release APK has the same certificate and can
// be installed over the previous version. Without the variables (plain local
// builds) we keep the historical debug-key fallback.
val releaseKeystorePath: String? = System.getenv("ANDROID_KEYSTORE_PATH")
val releaseKeystoreFile: java.io.File? =
    releaseKeystorePath?.let { path -> File(path).takeIf { it.isFile } }

android {
    namespace = "com.fusionplay.android"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
        isCoreLibraryDesugaringEnabled = true
    }

    defaultConfig {
        applicationId = "com.fusionplay.android"
        minSdk = flutter.minSdkVersion
        targetSdk = 36
        versionCode = flutter.versionCode
        versionName = flutter.versionName
        ndk {
            abiFilters += nativeAbis.toSet()
        }
        // The app has no translated resources beyond Chinese; keep the APK's
        // resource table small.
        resourceConfigurations += setOf("zh", "en")
    }

    signingConfigs {
        if (releaseKeystoreFile != null) {
            create("release") {
                storeFile = releaseKeystoreFile
                storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            // Stable release key when provided; otherwise keep the debug fallback
            // for local development (those debug APKs are not upgradeable).
            signingConfig = if (releaseKeystoreFile != null) {
                signingConfigs.getByName("release")
            } else {
                project.logger.warn(
                    "ANDROID_KEYSTORE_PATH is not set; release APK is signed with the debug key " +
                        "and cannot be installed as an upgrade over a release-key build."
                )
                signingConfigs.getByName("debug")
            }
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    buildFeatures {
        buildConfig = true
    }

    lint {
        disable += "NullSafeMutableLiveData"
    }

    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
        resources {
            excludes += setOf("META-INF/DEPENDENCIES", "META-INF/LICENSE*", "META-INF/NOTICE*")
        }
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDir(generatedNativeLibs)
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs_nio:2.1.5")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
    implementation("androidx.media3:media3-exoplayer:1.8.0")
    implementation("androidx.media:media:1.7.0")
    testImplementation("junit:junit:4.13.2")
}

tasks.withType<Test>().configureEach {
    maxParallelForks = 1
    maxHeapSize = "256m"
    jvmArgs("-XX:+UseSerialGC")
}

val nativeTriples = mapOf(
    "armeabi-v7a" to "arm-linux-androideabi",
    "arm64-v8a" to "aarch64-linux-android",
    "x86_64" to "x86_64-linux-android",
)

fun cargoExecutable(): String {
    val cargoHome = System.getenv("CARGO_HOME")
    val candidates = buildList {
        if (!cargoHome.isNullOrBlank()) {
            add("$cargoHome/bin/cargo.exe")
            add("$cargoHome/bin/cargo")
        }
        add("${System.getProperty("user.home")}/.cargo/bin/cargo.exe")
        add("${System.getProperty("user.home")}/.cargo/bin/cargo")
        add("cargo")
    }
    return candidates.first { path -> path == "cargo" || file(path).exists() }
}

fun ndkPath(): String {
    val configured = System.getenv("ANDROID_NDK_HOME") ?: System.getenv("NDK_HOME")
    if (!configured.isNullOrBlank()) {
        return configured
    }
    val sdk = System.getenv("ANDROID_HOME") ?: System.getenv("ANDROID_SDK_ROOT")
        ?: error("ANDROID_HOME or ANDROID_SDK_ROOT must point to an Android SDK.")
    return "$sdk/ndk/28.2.13676358"
}

val fusionPlayCargoHome = System.getenv("CARGO_HOME")
    ?.takeIf { it.isNotBlank() }
    ?: error("CARGO_HOME must point to a writable Cargo home directory.")

tasks.register("buildRustNative") {
    group = "build"
    description = "Cross-compiles FusionPlay's Rust receiver for Android ABIs"
    inputs.files(fileTree(repositoryRoot.dir("src/AirPlayReceiver.Core")) {
        include("Cargo.toml", "Cargo.lock", "build.rs", "src/**")
    })
    inputs.files(fileTree(repositoryRoot.dir("src/FusionPlay.MiPlaySdk")) {
        include("Cargo.toml", "Cargo.lock", "src/**")
    })
    inputs.files(fileTree(repositoryRoot.dir("src/FusionPlay.AndroidIfAddrs")) {
        include("Cargo.toml", "src/**")
    })
    inputs.files(fileTree(repositoryRoot.dir("vendor/shairplay")) {
        include("Cargo.toml", "src/**")
    })
    outputs.dir(generatedNativeLibs)
    doLast {
        val ndk = ndkPath()
        val cargo = cargoExecutable()
        val outputDirectory = generatedNativeLibs.get().asFile
        val prebuiltRoot = file("$ndk/toolchains/llvm/prebuilt")
        val toolchainHost = prebuiltRoot.listFiles()
            ?.firstOrNull { it.isDirectory }
            ?: error("Android NDK LLVM host toolchain was not found under $prebuiltRoot")
        val coreDirectory = repositoryRoot.dir("src/AirPlayReceiver.Core").asFile
        require(coreDirectory.resolve("Cargo.toml").isFile) {
            "Rust manifest was not found under $coreDirectory"
        }
        nativeAbis.forEach { abi ->
            exec {
                workingDir = coreDirectory
                environment("CARGO_HOME", fusionPlayCargoHome)
                environment("ANDROID_NDK_HOME", ndk)
                environment("ANDROID_NDK_ROOT", ndk)
                commandLine(
                    cargo,
                    "ndk",
                    "--target", abi,
                    "--platform", "21",
                    "--output-dir", outputDirectory.absolutePath,
                    "build",
                    "--release",
                    "--lib",
                )
            }
            val produced = outputDirectory.resolve("$abi/libfusionplay_core.so")
            require(produced.isFile) {
                "Native library was not produced for $abi at $produced"
            }
            val cxxRuntime = toolchainHost.resolve(
                "sysroot/usr/lib/${nativeTriples.getValue(abi)}/libc++_shared.so",
            )
            require(cxxRuntime.isFile) {
                "Android C++ runtime was not found at ${cxxRuntime.absolutePath}"
            }
            copy {
                from(cxxRuntime)
                into(outputDirectory.resolve(abi))
            }
        }
    }
}

tasks.matching {
    it.name.startsWith("merge") &&
        (it.name.endsWith("JniLibFolders") || it.name.contains("NativeLibs"))
}.configureEach {
    dependsOn("buildRustNative")
}

tasks.named("preBuild").configure {
    dependsOn("buildRustNative")
}
