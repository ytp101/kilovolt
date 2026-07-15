import { NextResponse } from 'next/server';

export async function GET(request: Request) {
    const { searchParams } = new URL(request.url);
    const version = searchParams.get('version') || '0.0.0';
    const isDocker = searchParams.get('is_docker') === 'true';
    const os = searchParams.get('os') || 'unknown';
    const arch = searchParams.get('arch') || 'unknown';

    // Diagnostic console trace on the Next.js backend
    console.log(`[Telemetry Log] Update check received from Kilovolt client:
  - Version: ${version}
  - Environment: ${isDocker ? 'Docker Container' : 'Native Host'}
  - Platform: OS: ${os} | Arch: ${arch}`);

    const latestVersion = '1.3.0';

    // Basic SemVer parts check utility
    const isOutdated = (current: string, target: string) => {
        const cParts = current.split('.').map(Number);
        const tParts = target.split('.').map(Number);
        for (let i = 0; i < 3; i++) {
            const cVal = cParts[i] || 0;
            const tVal = tParts[i] || 0;
            if (cVal < tVal) return true;
            if (cVal > tVal) return false;
        }
        return false;
    };

    const updateAvailable = isOutdated(version, latestVersion);

    return NextResponse.json({
        latest_version: latestVersion,
        update_available: updateAvailable,
        message: updateAvailable
            ? `Upgrade available: version ${latestVersion} is out. Please update for latest Gemini SSE stream translation features!`
            : "You are running the latest stable release of Kilovolt. All telemetry parameters verified."
    });
}
