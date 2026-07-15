import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { cookies } from 'next/headers';

const logFilePath = path.join(process.cwd(), 'telemetry_log.json');

export async function GET() {
    // 1. Session verification
    const cookieStore = await cookies();
    const session = cookieStore.get('admin_session')?.value;

    if (session !== 'authenticated') {
        return NextResponse.json({ error: 'Unauthorized access' }, { status: 401 });
    }

    // 2. Fetch logs from file
    try {
        const fileData = await fs.readFile(logFilePath, 'utf8');
        const logs = JSON.parse(fileData);
        return NextResponse.json(logs);
    } catch (e) {
        // Return empty array if file does not exist yet
        return NextResponse.json([]);
    }
}
