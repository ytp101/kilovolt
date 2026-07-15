import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';

const isVercel = process.env.VERCEL === '1';
const logFilePath = isVercel 
    ? path.join('/tmp', 'telemetry_log.json')
    : path.join(process.cwd(), 'telemetry_log.json');

async function saveTelemetryLog(data: {
    version: string;
    isDocker: boolean;
    os: string;
    arch: string;
    ip: string;
}) {
    try {
        let logs: any[] = [];
        try {
            const fileData = await fs.readFile(logFilePath, 'utf8');
            logs = JSON.parse(fileData);
        } catch (e) {
            // File does not exist yet, keep logs empty
        }
        
        logs.push({
            timestamp: new Date().toISOString(),
            ...data
        });

        // Cap at 1000 records to prevent storage bloat
        if (logs.length > 1000) {
            logs = logs.slice(logs.length - 1000);
        }

        await fs.writeFile(logFilePath, JSON.stringify(logs, null, 2), 'utf8');
    } catch (err) {
        console.error('[Telemetry Logger Error]', err);
    }
}

export async function GET(request: Request) {
    const { searchParams } = new URL(request.url);
    const version = searchParams.get('version') || '0.0.0';
    const isDocker = searchParams.get('is_docker') === 'true';
    const os = searchParams.get('os') || 'unknown';
    const arch = searchParams.get('arch') || 'unknown';
    
    // Attempt to extract client IP safely
    const ip = request.headers.get('x-forwarded-for') || '127.0.0.1';

    // Log diagnostic output
    console.log(`[Telemetry Log] Update check received from Kilovolt client:
  - Version: ${version}
  - Environment: ${isDocker ? 'Docker Container' : 'Native Host'}
  - Platform: OS: ${os} | Arch: ${arch}`);

    // Persist log entry asynchronously
    await saveTelemetryLog({ version, isDocker, os, arch, ip });

    const latestVersion = '1.3.0';

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
