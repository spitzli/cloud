import { afterAll,expect,mock,test } from 'bun:test';
import { act,createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { Window } from 'happy-dom';
mock.module('next/link',()=>({default:({prefetch,...props})=>createElement('a',{...props,'data-prefetch':String(prefetch)})}));
const {IntentLink}=await import('../../website/src/components/intent-link');
afterAll(()=>mock.restore());
test('Next links warm only the intended href on hover or keyboard focus and preserve link semantics',async()=>{
  const w=new Window({url:'https://site.example.test'});Object.assign(globalThis,{window:w,document:w.document,navigator:w.navigator,IS_REACT_ACT_ENVIRONMENT:true});const host=w.document.createElement('div');w.document.body.append(host);const root=createRoot(host);
  try{await act(()=>root.render(createElement(IntentLink,{href:'/privacy',className:'legal'},'Privacy')));let a=host.querySelector('a');expect(a.getAttribute('href')).toBe('/privacy');expect(a.getAttribute('class')).toBe('legal');expect(a.getAttribute('data-prefetch')).toBe('false');await act(()=>a.dispatchEvent(new w.MouseEvent('mouseover',{bubbles:true})));expect(a.getAttribute('data-prefetch')).toBe('true');
    await act(()=>root.render(createElement(IntentLink,{href:'/terms'},'Terms')));a=host.querySelector('a');expect(a.getAttribute('data-prefetch')).toBe('false');await act(()=>a.dispatchEvent(new w.FocusEvent('focusin',{bubbles:true})));expect(a.getAttribute('data-prefetch')).toBe('true');
  }finally{await act(()=>root.unmount());await w.happyDOM.close();}
});
test('prevented intent is honored instead of starting speculative work',async()=>{
  const w=new Window();Object.assign(globalThis,{window:w,document:w.document,navigator:w.navigator,IS_REACT_ACT_ENVIRONMENT:true});const host=w.document.createElement('div');const root=createRoot(host);try{await act(()=>root.render(createElement(IntentLink,{href:'/privacy',onMouseEnter:e=>e.preventDefault()},'Privacy')));const a=host.querySelector('a');await act(()=>a.dispatchEvent(new w.MouseEvent('mouseover',{bubbles:true,cancelable:true})));expect(a.getAttribute('data-prefetch')).toBe('false');}finally{await act(()=>root.unmount());await w.happyDOM.close();}
});
