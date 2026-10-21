import {Transformer} from '@parcel/plugin';
import {type CsfFile, enrichCsf, formatCsf, loadCsf} from 'storybook/internal/csf-tools';
import path from 'path';
import {getClient, getCacheDir} from './react-docgen-typescript';
import {ComponentDoc} from 'react-docgen-typescript';
import SourceMap, {type VLQMap} from '@parcel/source-map';

// What `StoryEdits.babel.ts` and `StoryEdits.estree.ts` export.
interface StoryEdits {
  formatOptions: object;
  editStories(
    csf: CsfFile,
    code: string,
    filePath: string,
    docs: ComponentDoc | null,
    refreshName: string | null,
    enrich: () => Promise<void>
  ): Promise<void>;
}

// Storybook 11 replaced the Babel nodes of Storybook 10's csf-tools with ESTree nodes and edits to
// the source text, so the story edits come in one module for each. Each works only on the
// Storybook it is written for, so only that one is required. `require` and not `import()`: Parcel
// compiles the imports of a plugin it loads from source, but leaves `import()` to Node, which does
// not find a `.ts` file without its extension.
function loadStoryEdits(): StoryEdits {
  let {parseModule} = require('storybook/internal/csf-tools');
  return typeof parseModule === 'function'
    ? require('./StoryEdits.estree')
    : require('./StoryEdits.babel');
}

module.exports = new Transformer({
  async transform({asset, options}) {
    let docs: ComponentDoc | null = null;
    if (asset.type === 'ts' || asset.type === 'tsx') {
      let client = await getClient(options);
      docs = await client.getDocs(asset.filePath);
    }
    let code = await asset.getCode();
    let name = options.hmrOptions ? `$parcel$ReactRefresh$${asset.id.slice(-4)}` : null;
    let {editStories, formatOptions} = loadStoryEdits();

    let csf = loadCsf(code, {
      fileName: asset.filePath,
      makeTitle: title => title || 'default'
    }).parse();
    // `enrichCsf` adds each story's `originalSource` and JSDoc description. The story edits run it
    // where their own edits need it.
    await editStories(csf, code, asset.filePath, docs, name, () => enrichCsf(csf, csf));
    let {code: compiledCode, map: compiledMap} = formatCsf(csf, {
      ...formatOptions,
      sourceFileName: asset.filePath,
      sourceMaps: true
    }) as {code: string; map: VLQMap};

    let map = new SourceMap(options.projectRoot);
    map.addVLQMap(compiledMap);

    // Invalidate the asset whenever any types change.
    asset.invalidateOnFileChange(path.join(getCacheDir(options), 'sentinel'));
    asset.setCode(compiledCode);
    asset.setMap(map);
    return [asset];
  }
});
