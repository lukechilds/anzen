// The browser emulator draws its own status bar and home indicator inside the phone it shows,
// and lays its screens out around them itself: there are no device insets to add.
const none = { top: 0, bottom: 0, left: 0, right: 0 };
export const useInsets = () => none;
