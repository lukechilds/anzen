// The browser emulator plays a simulated Face ID check of its own (see FaceId.tsx); this only
// names it.
export async function authenticationName(): Promise<string> { return 'Face ID'; }
