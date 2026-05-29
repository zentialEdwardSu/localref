import init from './localref-ui-bindgen.js';

try {
  await init();
} catch (error) {
  console.error('failed to initialize Localref WASM UI', error);
}
