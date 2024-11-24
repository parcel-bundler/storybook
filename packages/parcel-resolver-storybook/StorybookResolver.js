const path = require('path');
const { Resolver } = require("@parcel/plugin");
const reactVersion = require("react-dom/package.json").version;
const { default: NodeResolver } = require("@parcel/node-resolver-core");
const {isGlob, glob, normalizeSeparators, relativePath} = require('@parcel/utils');

const REACT_MAJOR_VERSION = parseInt(reactVersion.split('.')[0], 10);

module.exports = new Resolver({
  async resolve({ dependency, options, specifier, pipeline, logger }) {
    // Workaround for interop issue
    if (specifier === "react-dom/client") {
      let specifier = REACT_MAJOR_VERSION >= 18
        ? "react-dom/client.js"
        : "react-dom/index.js";
      return {
        filePath: __dirname + "/react.js",
        code: `
        export * from '${specifier}';
        export * as default from '${specifier}'
        `,
      };
    }

    // Resolve story entry globs. Storybook expects an object with relative paths from the process cwd as keys.
    // We do this in a resolver so that it invalidates the watcher when new stories are created.
    if (pipeline === 'story' && isGlob(specifier)) {
      let sourceFile = dependency.resolveFrom ?? dependency.sourcePath;
      let normalized = normalizeSeparators(path.resolve(path.dirname(sourceFile), specifier));
      let files = await glob(normalized, options.inputFS, {
        onlyFiles: true,
      });

      let cwd = process.cwd();
      let dir = path.dirname(sourceFile);
      let results = files.map(file => {
        let key = relativePath(cwd, file);
        let relative = relativePath(dir, file);
        return `  ${JSON.stringify(key)}: () => import(${JSON.stringify(relative)}),\n`;
      });

      return {
        filePath: path.join(
          dir,
          'stories.js'
        ),
        code: `module.exports = {\n${results.join('\n')}\n};\n`,
        invalidateOnFileCreate: [
          {glob: normalized}
        ],
        pipeline: null,
        priority: 'sync',
      };
    }

    // Workaround for pkg#exports support
    let rewritten;
    if (
      specifier.startsWith("@storybook/addon-essentials") &&
      specifier.endsWith("preview")
    ) {
      rewritten = specifier.replace(
        "@storybook/addon-essentials",
        "@storybook/addon-essentials/dist"
      );
    }
    if (rewritten) {
      const resolver = new NodeResolver({
        fs: options.inputFS,
        projectRoot: options.projectRoot,
        // Extensions are always required in URL dependencies.
        extensions:
          dependency.specifierType === "commonjs" ||
          dependency.specifierType === "esm"
            ? ["ts", "tsx", "mjs", "js", "jsx", "cjs", "json"]
            : [],
        mainFields: ["source", "browser", "module", "main"],
        packageManager: options.packageManager,
        shouldAutoInstall: options.shouldAutoInstall,
        logger,
      });

      return resolver.resolve({
        filename: rewritten,
        specifierType: dependency.specifierType,
        range: dependency.range,
        parent: dependency.resolveFrom,
        env: dependency.env,
        sourcePath: dependency.sourcePath,
        loc: dependency.loc,
      });
    }
  },
});
