import test from 'node:test';
import assert from 'node:assert/strict';
import { Window, type HTMLButtonElement as HappyButton, type HTMLInputElement as HappyInput } from 'happy-dom';
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { I18nProvider } from '@webdock/i18n/react';
import { HostingDirtyContext } from '../src/components/hosting/form-state';
import { ApplicationEnvironment } from '../src/components/hosting/environment';
import { ContainerArguments } from '../src/components/hosting/arguments';

test('removing a saved environment variable or container argument registers an unsaved change', async () => {
  const window = new Window();
  Object.assign(globalThis,{window,document:window.document,IS_REACT_ACT_ENVIRONMENT:true});
  const container=window.document.createElement('div');window.document.body.append(container);
  const root=createRoot(container as unknown as HTMLElement);
  let changed=0;
  try {
    await act(async()=>root.render(createElement(I18nProvider,{locale:"en",preference:"en"},createElement(HostingDirtyContext.Provider,{value:()=>changed++},createElement(ApplicationEnvironment,{names:['TOKEN']}),createElement(ContainerArguments,{values:['--verbose']})))));
    assert.equal(changed,0);
    await act(async()=>container.querySelector<HappyButton>('button[aria-label="Remove variable TOKEN"]')!.click());
    assert.equal(changed,1,'removing a saved secret must engage the navigation guard');
    assert.deepEqual(JSON.parse(container.querySelector<HappyInput>('input[name="environment"]')!.value),[{name:'TOKEN',value:null}]);
    await act(async()=>[...container.querySelectorAll('button')].find(button=>button.textContent==='Remove argument')!.click());
    assert.equal(changed,2,'removing an argument must engage the navigation guard');
    assert.equal(container.querySelector<HappyInput>('input[name="args"]')!.value,'[]');
  } finally { await act(async()=>root.unmount());await window.happyDOM.close(); }
});
