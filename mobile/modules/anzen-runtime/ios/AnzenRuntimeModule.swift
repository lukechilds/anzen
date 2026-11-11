import ExpoModulesCore
import Foundation
import Security
import UIKit

public final class AnzenRuntimeModule: Module {
  private let queue = DispatchQueue(label: "app.anzen.rust", qos: .userInitiated)
  // Benchmarks block for their whole run, so they never share the wallet's command queue.
  private let benchmarkQueue = DispatchQueue(label: "app.anzen.benchmark", qos: .userInitiated)
  // A Ledger review waits for its person, so the link has a queue of its own.
  private let ledgerQueue = DispatchQueue(label: "app.anzen.ledger", qos: .userInitiated)
  // As does a Trezor's.
  private let trezorQueue = DispatchQueue(label: "app.anzen.trezor", qos: .userInitiated)

  public func definition() -> ModuleDefinition {
    Name("AnzenRuntime")
    // For measuring on devices: launched with `--anzen-benchmark '{"engine":"gpu","seconds":5}'`
    // (`xcrun devicectl device process launch --console ...`), the app runs that benchmark once
    // the scene is up, prints the result on stdout and exits. Public inputs only, as below.
    OnCreate {
      // Text cursors and selections are the app's mint. Each field asks for it too, but a field
      // focused as its sheet arrives sometimes shows the system's blue: this is what it falls
      // back to, and what the system's own prompts (a pairing code, say) use.
      DispatchQueue.main.async {
        let mint = UIColor(red: 167 / 255, green: 227 / 255, blue: 187 / 255, alpha: 1)
        UITextField.appearance().tintColor = mint
        UITextView.appearance().tintColor = mint
      }
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
      let root = try self.devicesRoot()
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
    // The Bluetooth link to a Ledger signer. Rust owns the connection; each call is one step.
    AsyncFunction("ledger") { (request: String) throws -> String in
      return try request.withCString { requestPointer in
        guard let pointer = anzen_mobile_ledger(requestPointer) else {
          throw self.failure("Rust returned no response")
        }
        defer { anzen_mobile_free(pointer) }
        return String(cString: pointer)
      }
    }.runOnQueue(ledgerQueue)
    // The link to a Trezor signer. Rust owns the connection, and keeps this phone's pairing with
    // the Trezor and the signer's sealed state beside the devices: protected while the phone is
    // locked, and never in a backup.
    AsyncFunction("trezor") { (request: String) throws -> String in
      let root = try self.devicesRoot()
      return try root.path.withCString { rootPointer in
        try request.withCString { requestPointer in
          guard let pointer = anzen_mobile_trezor(rootPointer, requestPointer) else {
            throw self.failure("Rust returned no response")
          }
          defer { anzen_mobile_free(pointer) }
          return String(cString: pointer)
        }
      }
    }.runOnQueue(trezorQueue)
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

  // Where this phone's devices are stored: unreadable while the phone is locked, and excluded
  // from backups.
  private func devicesRoot() throws -> URL {
    let manager = FileManager.default
    var root = try manager.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
      .appendingPathComponent("AnzenDevices", isDirectory: true)
    try manager.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete])
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try root.setResourceValues(values)
    return root
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
    // A role's devices live at root/<network>/<role>, or at root/<role> from before networks
    // were a setting.
    let devices = [role] + ["signet", "regtest", "bitcoin"].map { "\($0)/\(role)" }
    guard !devices.contains(where: { FileManager.default.fileExists(atPath: root.appendingPathComponent($0).path) }) else {
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
