# ==============================================================================
# MANTA BROWSER — ULTRA-AGGRESSIVE R8 OPTIMIZATION & SHRINKING RULES
# ==============================================================================
-optimizationpasses 5
-allowaccessmodification
-repackageclasses 'm'
-dontusemixedcaseclassnames
-dontskipnonpubliclibraryclasses
-dontpreverify

# Strip all debug logging in release builds
-assumenosideeffects class android.util.Log {
    public static boolean isLoggable(java.lang.String, int);
    public static int v(...);
    public static int d(...);
    public static int i(...);
}

# Preserve JNI bridge methods called from Rust `libmanta_core.so`
-keepclasseswithmembernames class app.manta.browser.core.MantaNative {
    native <methods>;
}

-keep class app.manta.browser.MainActivity {
    public <init>();
    protected void onCreate(android.os.Bundle);
}
