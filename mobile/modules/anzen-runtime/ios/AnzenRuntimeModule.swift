import ExpoModulesCore
import Foundation
import Security
import UIKit

public final class AnzenRuntimeModule: Module {
  private let queue = DispatchQueue(label: "app.anzen.rust", qos: .userInitiated)
  // Benchmarks block for their whole run, so they never share the wallet's command queue.
  private let benchmarkQueue = DispatchQueue(label: "app.anzen.benchmark", qos: .userInitiated)

  public func definition() -> ModuleDefinition {
    Name("AnzenRuntime")
    // For measuring on devices: launched with `--anzen-benchmark '{"engine":"gpu","seconds":5}'`
    // (`xcrun devicectl device process launch --console ...`), the app runs that benchmark once
    // the scene is up, prints the result on stdout and exits. Public inputs only, as below.
    OnCreate {
      let arguments = ProcessInfo.processInfo.arguments
      guard let at = arguments.firstIndex(of: "--anzen-benchmark"), at + 1 < arguments.count else { return }
      let request = arguments[at + 1]
      self.benchmarkQueue.asyncAfter(deadline: .now() + 1) {
        DispatchQueue.main.sync { UIApplication.shared.isIdleTimerDisabled = true }
        let response = request.withCString { requestPointer -> String in
          guard let pointer = anzen_mobile_benchmark(requestPointer) else { return "null" }
          defer { anzen_mobile_free(pointer) }
          return String(cString: pointer)
        }
        print("ANZEN_BENCHMARK \(response)")
        fflush(stdout)
        exit(0)
      }
    }
    AsyncFunction("execute") { (role: String, command: String, settings: String) throws -> String in
      guard role == "wallet" || role == "signer" else { throw self.failure("Invalid device role") }
      let manager = FileManager.default
      var root = try manager.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        .appendingPathComponent("AnzenDevices", isDirectory: true)
      try manager.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete])
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try root.setResourceValues(values)
      var key = try self.storageKey(role: role, root: root)
      defer { key.resetBytes(in: 0..<key.count) }
      return try root.path.withCString { rootPointer in
        try role.withCString { rolePointer in
          try settings.withCString { settingsPointer in
            try command.withCString { commandPointer in
              try key.withUnsafeBytes { keyBytes in
                guard let pointer = anzen_mobile_execute(rootPointer, rolePointer, keyBytes.bindMemory(to: UInt8.self).baseAddress, settingsPointer, commandPointer) else {
                  throw self.failure("Rust returned no response")
                }
                defer { anzen_mobile_free(pointer) }
                return String(cString: pointer)
              }
            }
          }
        }
      }
    }.runOnQueue(queue)
    // GPU benchmarks on fixed public inputs; no keys or storage. iOS stops GPU work once the
    // app leaves the foreground, so the screen stays awake while one runs.
    AsyncFunction("benchmark") { (request: String) throws -> String in
      DispatchQueue.main.sync { UIApplication.shared.isIdleTimerDisabled = true }
      defer { DispatchQueue.main.async { UIApplication.shared.isIdleTimerDisabled = false } }
      return try request.withCString { requestPointer in
        guard let pointer = anzen_mobile_benchmark(requestPointer) else {
          throw self.failure("Rust returned no response")
        }
        defer { anzen_mobile_free(pointer) }
        return String(cString: pointer)
      }
    }.runOnQueue(benchmarkQueue)
    // Live updates run on Rust's own thread; this call only starts, updates or reads them, so it
    // is synchronous and never waits behind a command on the queue above.
    Function("watch") { (settings: String, request: String) throws -> String in
      return try settings.withCString { settingsPointer in
        try request.withCString { requestPointer in
          guard let pointer = anzen_mobile_watch(settingsPointer, requestPointer) else {
            throw self.failure("Rust returned no response")
          }
          defer { anzen_mobile_free(pointer) }
          return String(cString: pointer)
        }
      }
    }
  }

  // The storage key never leaves native memory. It is device-bound, excluded from backups,
  // and unavailable while locked. This does not turn a phone into a hardware secure element.
  private func storageKey(role: String, root: URL) throws -> Data {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.anzen.storage.v1",
      kSecAttrAccount as String: role,
      kSecReturnData as String: true,
      kSecMatchLimit as String: kSecMatchLimitOne
    ]
    var result: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &result)
    if status == errSecSuccess {
      guard let data = result as? Data, data.count == 32 else { throw failure("Invalid storage key") }
      return data
    }
    guard status == errSecItemNotFound else { throw failure("Unlock the device to access Anzen storage (\(status))") }
    guard !FileManager.default.fileExists(atPath: root.appendingPathComponent(role).path) else {
      throw failure("This device's storage key is missing. Restore from a recovery backup; it will not be replaced automatically.")
    }
    var key = Data(count: 32)
    let randomStatus = key.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 32, $0.baseAddress!) }
    guard randomStatus == errSecSuccess else { throw failure("Secure random generation failed") }
    let item: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.anzen.storage.v1",
      kSecAttrAccount as String: role,
      kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
      kSecValueData as String: key
    ]
    let added = SecItemAdd(item as CFDictionary, nil)
    guard added == errSecSuccess else { throw failure("Could not protect storage key (\(added))") }
    return key
  }

  private func failure(_ message: String) -> NSError {
    NSError(domain: "AnzenRuntime", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
  }
}
