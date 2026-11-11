// The phone's taps and buzzes, for moments that deserve one: a choice made, a payment sent, an
// approval held to the end, something refused. Failures to play are ignored: a device with
// haptics turned off is not an error.
import * as Haptics from 'expo-haptics';

const quiet = (play: Promise<void>) => { play.catch(() => {}); };

export const haptics = {
  // A selection changed, e.g. a tab.
  select: () => quiet(Haptics.selectionAsync()),
  // A press that begins something deliberate, e.g. holding to approve.
  press: () => quiet(Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium)),
  success: () => quiet(Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success)),
  failure: () => quiet(Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error)),
};
