#include <jni.h>
#include <array>
#include <codecvt>
#include <cstring>
#include <locale>
#include <string>

extern "C" char *anzen_mobile_execute(const char *, const char *, const unsigned char *, const char *, const char *);
extern "C" char *anzen_mobile_watch(const char *, const char *);
extern "C" void anzen_mobile_free(char *);

// JNI strings are UTF-16. Do not pass modified UTF-8 to Rust's strict UTF-8 decoder.
static std::string utf8(JNIEnv *env, jstring value) {
  const auto length = env->GetStringLength(value);
  const auto chars = env->GetStringChars(value, nullptr);
  if (!chars) throw std::runtime_error("Could not read native string");
  const std::u16string copy(reinterpret_cast<const char16_t *>(chars), length);
  env->ReleaseStringChars(value, chars);
  auto text = std::wstring_convert<std::codecvt_utf8_utf16<char16_t>, char16_t>{}.to_bytes(copy);
  if (text.find('\0') != std::string::npos) throw std::runtime_error("NUL in native argument");
  return text;
}

extern "C" JNIEXPORT jbyteArray JNICALL
Java_expo_modules_anzen_AnzenRuntimeModule_executeRust(JNIEnv *env, jobject,
  jstring root, jstring role, jbyteArray key, jstring settings, jstring command) {
  struct KeyBytes {
    std::array<unsigned char, 32> value{};
    ~KeyBytes() { volatile auto *p = value.data(); for (size_t i = 0; i < value.size(); ++i) p[i] = 0; }
  } secret;
  try {
    if (!root || !role || !key || !settings || !command || env->GetArrayLength(key) != 32)
      throw std::runtime_error("Invalid native arguments");
    env->GetByteArrayRegion(key, 0, 32, reinterpret_cast<jbyte *>(secret.value.data()));
    if (env->ExceptionCheck()) return nullptr;
    const auto rootText = utf8(env, root), roleText = utf8(env, role);
    const auto settingsText = utf8(env, settings), commandText = utf8(env, command);
    char *result = anzen_mobile_execute(rootText.c_str(), roleText.c_str(), secret.value.data(), settingsText.c_str(), commandText.c_str());
    if (!result) throw std::runtime_error("Rust returned no response");
    const auto length = static_cast<jsize>(std::strlen(result));
    auto bytes = env->NewByteArray(length);
    if (bytes) env->SetByteArrayRegion(bytes, 0, length, reinterpret_cast<jbyte *>(result));
    anzen_mobile_free(result);
    return bytes;
  } catch (const std::exception &error) {
    if (!env->ExceptionCheck()) env->ThrowNew(env->FindClass("java/lang/IllegalStateException"), error.what());
    return nullptr;
  }
}

extern "C" JNIEXPORT jbyteArray JNICALL
Java_expo_modules_anzen_AnzenRuntimeModule_watchRust(JNIEnv *env, jobject, jstring settings, jstring request) {
  try {
    if (!settings || !request) throw std::runtime_error("Invalid native arguments");
    const auto settingsText = utf8(env, settings), requestText = utf8(env, request);
    char *result = anzen_mobile_watch(settingsText.c_str(), requestText.c_str());
    if (!result) throw std::runtime_error("Rust returned no response");
    const auto length = static_cast<jsize>(std::strlen(result));
    auto bytes = env->NewByteArray(length);
    if (bytes) env->SetByteArrayRegion(bytes, 0, length, reinterpret_cast<jbyte *>(result));
    anzen_mobile_free(result);
    return bytes;
  } catch (const std::exception &error) {
    if (!env->ExceptionCheck()) env->ThrowNew(env->FindClass("java/lang/IllegalStateException"), error.what());
    return nullptr;
  }
}
