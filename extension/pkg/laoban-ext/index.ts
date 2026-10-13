import { importTypes } from '@rancher/auto-import';
import { IPlugin } from '@shell/core/types';
import extensionRouting from './routing/extension-routing';
import { EXTRA_LOCALES } from './config/laoban';

// Init the package
export default function(plugin: IPlugin): void {
  // Auto-import model, detail, edit from the folders.
  // This also registers every ./l10n/<locale>.yaml file as translations for <locale>.
  importTypes(plugin);

  // Provide plugin metadata from package.json
  plugin.metadata = require('./package.json');

  // Make locales that Rancher does not ship selectable (see config/laoban.ts)
  EXTRA_LOCALES.forEach(({ locale, label }) => plugin.addLocale(locale, label));

  // Load a product
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  plugin.addProduct(require('./product'));

  // Add Vue Routes
  plugin.addRoutes(extensionRouting);
}
