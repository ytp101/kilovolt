import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { cookies } from 'next/headers';

const isVercel = process.env.VERCEL === '1';
const logFilePath = isVercel 
    ? path.join('/tmp', 'telemetry_log.json')
    : path.join(process.cwd(), 'telemetry_log.json');

const analyticsFilePath = isVercel
    ? path.join('/tmp', 'telemetry_analytics.json')
    : path.join(process.cwd(), 'telemetry_analytics.json');

export async function GET() {
    // 1. Session verification
    const cookieStore = await cookies();
    const session = cookieStore.get('admin_session')?.value;

    if (session !== 'authenticated') {
        return NextResponse.json({ error: 'Unauthorized access' }, { status: 401 });
    }

    // 2. Fetch logs from file
    let logs: any[] = [];
    try {
        const fileData = await fs.readFile(logFilePath, 'utf8');
        logs = JSON.parse(fileData);
    } catch (e) {
        // Return empty logs if file doesn't exist
    }

    // 3. Fetch analytics from file
    let analytics = {
        total_spend_under_management: 0.0,
        total_requests_managed: 0,
        total_tokens_managed: 0,
        active_instances: [] as string[]
    };
    try {
        const fileData = await fs.readFile(analyticsFilePath, 'utf8');
        analytics = JSON.parse(fileData);
    } catch (e) {
        // Return empty analytics if file doesn't exist
    }

    const storageType = isVercel ? 'Vercel Serverless /tmp (Ephemeral)' : 'Local Disk (Persistent)';

    return NextResponse.json({ logs, analytics, storage_type: storageType });
}
