plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Automatically sync root `logo.png` into `src/main/res/drawable-nodpi/logo.png`
// so Android uses `logo.png` directly as `@drawable/logo` without any vector drawable.
val syncRootLogo by tasks.registering(Copy::class) {
    from(rootProject.file("logo.png"))
    into(layout.projectDirectory.dir("src/main/res/drawable-nodpi"))
}

tasks.matching { it.name == "preBuild" }.configureEach {
    dependsOn(syncRootLogo)
}

android {
    namespace = "app.manta.browser"
    compileSdk = 34

    defaultConfig {
        applicationId = "app.manta.browser"
        minSdk = 26
        targetSdk = 34
        versionCode = 3
        versionName = "1.1.0-core"

        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            signingConfig = signingConfigs.getByName("debug")
        }
        debug {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
        resources {
            excludes += setOf(
                "META-INF/**",
                "kotlin/**",
                "**.bin",
                "DebugProbesKt.bin"
            )
        }
    }
}

dependencies {
    // Zero Google Play Services, Zero AndroidX bloat, Zero third-party tracking SDKs.
}
