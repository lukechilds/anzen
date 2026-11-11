// Confirming the owner is present, with the device's own check: Face ID or Touch ID, falling
// back to the passcode. This guards what the app shows and starts; the keys' protection is
// Rust's and the keychain's, not this.
import * as LocalAuthentication from 'expo-local-authentication';

// Resolves true once the owner is confirmed. A phone with no passcode at all has nothing to ask
// with, and anyone holding it is already in: it passes.
export async function authenticate(): Promise<boolean> {
  const level = await LocalAuthentication.getEnrolledLevelAsync();
  if (level === LocalAuthentication.SecurityLevel.NONE) return true;
  const result = await LocalAuthentication.authenticateAsync({ promptMessage: 'Unlock Anzen', cancelLabel: 'Cancel' });
  return result.success;
}

// What the device's check is called, for the app's words about it.
export async function authenticationName(): Promise<string> {
  const types = await LocalAuthentication.supportedAuthenticationTypesAsync();
  if (types.includes(LocalAuthentication.AuthenticationType.FACIAL_RECOGNITION)) return 'Face ID';
  if (types.includes(LocalAuthentication.AuthenticationType.FINGERPRINT)) return 'Touch ID';
  return 'your passcode';
}
