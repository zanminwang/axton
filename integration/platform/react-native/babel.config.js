// The Expo preset applies its TypeScript transform to .ts/.tsx names only.
// The shared AXTON client modules are .mts, so give them the same transform.
const expoPackage = require.resolve('expo/package.json');
const resolveFromExpo = (name) => require.resolve(name, { paths: [expoPackage] });

module.exports = function (api) {
  api.cache(true);
  return {
    presets: [resolveFromExpo('babel-preset-expo')],
    overrides: [
      {
        test: (filename) => !!filename && filename.endsWith('.mts'),
        plugins: [
          [resolveFromExpo('@babel/plugin-transform-typescript'), { isTSX: false, allowNamespaces: true }],
        ],
      },
    ],
  };
};
