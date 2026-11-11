import { ImageResponse } from 'next/og';
import logo from '../../../public/spitzli-logo.png?inline';
import fontData from '../../../public/fonts/spitzli-cloud-og-medium.ttf?inline';

export const dynamic = 'force-static';
export const runtime = 'nodejs';
const colors = { paper: '#141318', ink: '#F5F3F9', muted: '#B4AFBE', accent: '#C6A3FF' };
export async function GET() {
  const font = Buffer.from(fontData.slice(fontData.indexOf(',') + 1), 'base64');
  return new ImageResponse(
    <div style={{ width: '100%', height: '100%', display: 'flex', flexDirection: 'column', justifyContent: 'space-between', padding: 64, background: colors.paper, color: colors.ink, fontFamily: 'Spitzli Cloud OG Sans' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 24 }}>
        {/* Existing owned logo bytes, unchanged. */}
        {/* eslint-disable-next-line @next/next/no-img-element -- ImageResponse renders its own embedded image. */}
        <img src={logo} alt="" width={80} height={80}/>
        <span style={{ fontSize: 42 }}>Spitzli Cloud</span>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20 }}>
        <span style={{ fontSize: 84, letterSpacing: -4 }}>Your projects. One place.</span>
        <span style={{ fontSize: 30, color: colors.muted }}>Websites, applications and cloud services.</span>
      </div>
      <span style={{ fontSize: 28, color: colors.accent }}>spitzli.cloud</span>
    </div>,
    { width: 1200, height: 630, fonts: [{ name: 'Spitzli Cloud OG Sans', data: font, weight: 500, style: 'normal' }] },
  );
}
