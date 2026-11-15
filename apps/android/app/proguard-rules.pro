# kotlinx.serialization models are kept by the library's own consumer rules.
-keepattributes *Annotation*, InnerClasses
-dontwarn kotlinx.serialization.**
# JNA (UniFFI) once the RustEngine bindings are wired.
-keep class com.sun.jna.** { *; }
-keep class io.atomik.kb.engine.ffi.** { *; }
# JNA references AWT for desktop JVMs only; it is never loaded on Android.
-dontwarn java.awt.**
