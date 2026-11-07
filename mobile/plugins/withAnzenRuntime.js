const { withGradleProperties } = require('expo/config-plugins');
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

module.exports = function withAnzenRuntime(config) {
  config.ios = { ...config.ios, deploymentTarget: targets.ios.deploymentTarget,
    infoPlist: {
      NSLocalNetworkUsageDescription: 'Anzen connects to your Bitcoin node for regtest wallet testing.',
      ...config.ios?.infoPlist,
    },
  };
  return withGradleProperties(config, config => {
    config.modResults = configureProperties(config.modResults);
    return config;
  });
};
module.exports.configureProperties = configureProperties;
