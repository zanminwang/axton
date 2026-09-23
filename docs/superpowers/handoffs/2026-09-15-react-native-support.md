# React Native Support Handoff (#100)

## State

[React Native support #100](https://github.com/zanminwang/axton/issues/100) is implemented on branch `codex/react-native` in the worktree `/Users/stevewang/Github/local first state/.worktrees/react-native` (base `9b81fad`). It blocks [To-do demo #31](https://github.com/zanminwang/axton/issues/31), which consumes the SDK and does not implement platform support itself. Scope and file ownership: [spec](../specs/2026-09-15-react-native-support.md); steps and status: [plan](../plans/2026-09-15-react-native-support.md).

Verified on 2026-09-15 (arm64 iOS 26.5 simulator, Xcode 26.5, Expo 57.0.22, React Native 0.86.3): the Release app with embedded JavaScript builds, the two-simulator runner passes its offline/restart/reconnect/lost-response scenario, and `bash scripts/test.sh` passes. The single evidence record is the [harness README](../../../integration/platform/react-native/README.md); the package limits are in the [package README](../../../packages/client-react-native/README.md).

## Fixes made after the first implementation pass

- `super.init()` in the Swift exception subclass (the Release build failed to compile without it).
- Simulator builds are arm64-only (`ARCHS=arm64`, `EXCLUDED_ARCHS[sdk=iphonesimulator*]=x86_64`) because the vendored Rust library carries that slice.
- The podspec moved to `native-module/ios/` so Expo autolinking registers `AxtonNativeModule`; at the package root it linked through React Native's autolinking only and JavaScript failed with "Cannot find native module".
- `packages/client-react-native/plugins/expo-path-spaces` also quotes the React Native bundle phase, and `babel.config.js` applies the TypeScript transform to `.mts` files.
- The compiler emits `NameModel<P extends ReadPort>` instead of `declare` class fields, which Babel 7 presets reject.

## For the To-do demo (#31)

Consume `packages/client-react-native/index.ts` as the `--client-runtime` target, depend on `@axton/client-react-native` and `@axton/native` as file packages, reuse the harness's `metro.config.js` and `babel.config.js` and the package's `plugins/expo-path-spaces` config plugin, and build the Rust simulator slice with `bash packages/client-react-native/native-module/scripts/build-ios.sh simulator` before `pod install`. Use the user's CocoaPods on this machine (`~/.gem/ruby/3.1.3/bin/pod`); the Homebrew shim fails under Ruby 4.
