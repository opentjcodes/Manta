# ==============================================================================
# MANTA BROWSER — SAFE R8 OPTIMIZATION & JNI / JS-BRIDGE PRESERVATION RULES
# ==============================================================================
-optimizationpasses 5
-dontusemixedcaseclassnames
-dontskipnonpubliclibraryclasses
-dontpreverify
-keepattributes *Annotation*,JavascriptInterface,InnerClasses,EnclosingMethod,Signature

# Strip debug logging in release builds
-assumenosideeffects class android.util.Log {
    public static boolean isLoggable(java.lang.String, int);
    public static int v(...);
    public static int d(...);
}

# Unconditionally preserve MantaNative JNI class & methods so JNI linkage never fails
-keep class app.manta.browser.core.MantaNative {
    *;
}

-keepclasseswithmembernames class * {
    native <methods>;
}

# Preserve @JavascriptInterface methods on MantaJsBridge for Rust runtime script calls
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}

-keep class app.manta.browser.ui.render.EphemeralWebSandbox$MantaJsBridge {
    *;
}

# Preserve Activity and custom Views
-keep class app.manta.browser.MainActivity {
    *;
}

-keep class app.manta.browser.ui.** {
    *;
}
