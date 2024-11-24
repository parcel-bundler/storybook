# Storybook Builder Parcel

A [Storybook Builder](https://storybook.js.org/docs/builders) integration that uses Parcel as the bundler.

* Custom transformer to support [React Fast Refresh](https://reactnative.dev/docs/fast-refresh) in CSF story files. This enables you to edit your components **or stories** without losing state.
* Support for auto-generating controls and docs from TypeScript via [react-docgen-typescript](https://github.com/styleguidist/react-docgen-typescript), with hot reloading when your types change.
* Enhances CSF with automatic annotation of original story source code and description

## Setup

1. Install the dependencies:

```
npm install --save-dev storybook-react-parcel parcel-config-storybook
```

2. Create a Parcel config file in your Storybook folder, e.g. `.storybook/.parcelrc`:

```json
{
  "extends": "parcel-config-storybook"
}
```

You can add additional Parcel configuration here if needed.

3. Configure Storybook to use Parcel in `.storybook/main.js` (or TS):

```js
module.exports = {
  // ...
  framework: 'storybook-react-parcel'
};
```

See the example in this repo for more details.

## Acknowledgements

The code in this repo is based on the official builder plugins for Webpack and Vite, initially ported to Parcel by [Niklas Mischkulnig](https://github.com/mischnic).
