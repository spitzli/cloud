'use client';
import Link from 'next/link';
import { useState,type ComponentProps } from 'react';

type Props=Omit<ComponentProps<typeof Link>,'href'|'prefetch'>&{href:string};
/** Public same-origin navigation: warm on intent instead of loading every visible destination. */
export function IntentLink({href,onMouseEnter,onFocus,...props}:Props){
  const [intendedHref,setIntendedHref]=useState<string>();
  return <Link {...props} href={href} prefetch={intendedHref===href}
    onMouseEnter={event=>{onMouseEnter?.(event);if(!event.defaultPrevented)setIntendedHref(href);}}
    onFocus={event=>{onFocus?.(event);if(!event.defaultPrevented)setIntendedHref(href);}}/>;
}
