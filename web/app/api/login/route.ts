import { NextResponse } from 'next/server';

export async function POST(request: Request) {
    try {
        const { password } = await request.json();
        const adminPassword = process.env.ADMIN_PASSWORD || 'admin123';

        if (password === adminPassword) {
            const response = NextResponse.json({ success: true });
            
            // Set session cookie securely
            response.cookies.set('admin_session', 'authenticated', {
                path: '/',
                httpOnly: true,
                secure: process.env.NODE_ENV === 'production',
                sameSite: 'lax',
                maxAge: 86400 // 24 hours
            });

            return response;
        }

        return NextResponse.json({ error: 'Invalid password' }, { status: 401 });
    } catch (e) {
        return NextResponse.json({ error: 'Invalid request payload' }, { status: 400 });
    }
}
