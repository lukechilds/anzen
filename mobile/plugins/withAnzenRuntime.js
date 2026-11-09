const { withAppDelegate, withGradleProperties, withInfoPlist } = require('expo/config-plugins');
const targets = require('../native-targets.json');

// Build/platform configuration only. All wallet behavior remains in Rust.
function configureProperties(properties) {
  const minSdkValues = properties.filter(item => item.type === 'property' && item.key === 'android.minSdkVersion')
    .map(item => Number(item.value));
  if (minSdkValues.some(value => !Number.isSafeInteger(value) || value < 1)) {
    throw new Error('Invalid android.minSdkVersion; expected a positive API level.');
  }
  const managed = {
    reactNativeArchitectures: targets.android.targets.map(target => target.abi).join(','),
    'android.minSdkVersion': String(Math.max(targets.android.minSdk, ...minSdkValues)),
  };
  const result = properties.filter(item => item.type !== 'property' || !Object.hasOwn(managed, item.key));
  for (const [key, value] of Object.entries(managed)) result.push({ type: 'property', key, value });
  return result;
}

// iOS 27 stops apps built with its SDK at launch unless they use the scene lifecycle, which the
// Expo SDK 57 template does not. React Native starts in the window scene instead of the app
// delegate's window, and links reach the app delegate as before.
const appDelegateWindow = `#if os(iOS) || os(tvOS)
    window = UIWindow(frame: UIScreen.main.bounds)
    factory.startReactNative(
      withModuleName: "main",
      in: window,
      launchOptions: launchOptions)
#endif
`;
const sceneDelegate = `
class SceneDelegate: UIResponder, UIWindowSceneDelegate {
  var window: UIWindow?

  func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options connectionOptions: UIScene.ConnectionOptions) {
    guard let windowScene = scene as? UIWindowScene,
          let appDelegate = UIApplication.shared.delegate as? AppDelegate else { return }
    let window = UIWindow(windowScene: windowScene)
    self.window = window
    appDelegate.window = window
    var launchOptions: [UIApplication.LaunchOptionsKey: Any] = [:]
    if let url = connectionOptions.urlContexts.first?.url { launchOptions[.url] = url }
    appDelegate.reactNativeFactory?.startReactNative(withModuleName: "main", in: window, launchOptions: launchOptions)
  }

  func scene(_ scene: UIScene, openURLContexts contexts: Set<UIOpenURLContext>) {
    guard let appDelegate = UIApplication.shared.delegate as? AppDelegate else { return }
    for context in contexts { _ = appDelegate.application(UIApplication.shared, open: context.url) }
  }

  func scene(_ scene: UIScene, continue userActivity: NSUserActivity) {
    guard let appDelegate = UIApplication.shared.delegate as? AppDelegate else { return }
    _ = appDelegate.application(UIApplication.shared, continue: userActivity) { _ in }
  }
}
`;

function adoptSceneLifecycle(source) {
  if (source.includes('class SceneDelegate')) return source;
  if (!source.includes(appDelegateWindow)) {
    throw new Error('AppDelegate.swift no longer matches the Expo template; update the scene delegate in withAnzenRuntime.');
  }
  return source.replace(appDelegateWindow, '    // SceneDelegate starts React Native in the window scene.\n') + sceneDelegate;
}

const sceneManifest = {
  UIApplicationSupportsMultipleScenes: false,
  UISceneConfigurations: {
    UIWindowSceneSessionRoleApplication: [{
      UISceneConfigurationName: 'Default Configuration',
      UISceneDelegateClassName: '$(PRODUCT_MODULE_NAME).SceneDelegate',
    }],
  },
};

module.exports = function withAnzenRuntime(config) {
  config.ios = { ...config.ios, deploymentTarget: targets.ios.deploymentTarget,
    infoPlist: {
      NSLocalNetworkUsageDescription: 'Anzen connects to your Bitcoin node for regtest wallet testing.',
      ...config.ios?.infoPlist,
    },
  };
  config = withInfoPlist(config, config => {
    config.modResults.UIApplicationSceneManifest = sceneManifest;
    return config;
  });
  config = withAppDelegate(config, config => {
    if (config.modResults.language !== 'swift') throw new Error('The scene delegate expects a Swift AppDelegate.');
    config.modResults.contents = adoptSceneLifecycle(config.modResults.contents);
    return config;
  });
  return withGradleProperties(config, config => {
    config.modResults = configureProperties(config.modResults);
    return config;
  });
};
module.exports.configureProperties = configureProperties;
module.exports.adoptSceneLifecycle = adoptSceneLifecycle;
