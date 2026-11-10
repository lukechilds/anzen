require 'json'
runtime_targets = JSON.parse(File.read(File.join(__dir__, '../../../native-targets.json')))

Pod::Spec.new do |s|
  s.name = 'AnzenRuntime'
  s.version = '0.1.0'
  s.summary = 'Local Anzen Rust device runtime'
  s.description = 'UI transport and Keychain adapter for the Anzen role facades.'
  s.license = { :type => 'MIT' }
  s.author = 'Anzen contributors'
  s.homepage = 'https://github.com/lukechilds/anzen'
  s.source = { :git => 'https://github.com/lukechilds/anzen.git' }
  s.platforms = { :ios => runtime_targets.fetch('ios').fetch('deploymentTarget') }
  s.swift_version = '5.9'
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.source_files = '*.swift', 'include/*.h'
  s.public_header_files = 'include/AnzenRuntime.h'
  s.vendored_frameworks = 'AnzenMobile.xcframework'
  # Metal and QuartzCore for the GPU benchmarks (wgpu and native Metal); CoreBluetooth for the
  # link to a Ledger or Trezor signer.
  s.frameworks = 'Security', 'SystemConfiguration', 'Metal', 'QuartzCore', 'UIKit', 'CoreBluetooth'
  s.libraries = 'c++', 'resolv'
end
