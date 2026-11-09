import { Router } from 'express';
import { compare, hash } from 'bcrypt';
import jwt from 'jsonwebtoken';
import rateLimit from 'express-rate-limit';
import { db } from '../db';
import { JWT_SECRET } from '../config';
import { authMiddleware, requirePermission } from '../middleware/auth';
import { Permissions } from '../permissions';
import { User } from '../db/types';

const router = Router();

// Brute-force protection for credential endpoints. Keyed by client IP
// (trust proxy is enabled in index.ts so X-Forwarded-For is honoured).
const credentialLimiter = rateLimit({
  windowMs: 15 * 60 * 1000,
  max: 20,
  standardHeaders: true,
  legacyHeaders: false,
  message: { error: 'Too many attempts, try again later' }
});

// Password hashing is deliberately expensive; keep registration cheap to abuse.
const registerLimiter = rateLimit({
  windowMs: 60 * 60 * 1000,
  max: 10,
  standardHeaders: true,
  legacyHeaders: false,
  message: { error: 'Too many registrations from this address, try again later' }
});

router.post('/register', registerLimiter, async (req, res) => {
  const { username, password } = req.body;

  if (!username || !password) {
    return res.status(400).json({ error: 'Username and password required' });
  }

  if (typeof username !== 'string' || username.length < 3 || username.length > 32 || !/^[a-zA-Z0-9_.-]+$/.test(username)) {
    return res.status(400).json({ error: 'Username must be 3-32 characters (letters, numbers, . _ - only)' });
  }

  if (typeof password !== 'string' || password.length < 8) {
    return res.status(400).json({ error: 'Password must be at least 8 characters' });
  }

  try {
    const existingUser = await db.users.getUserByUsername(username);
    if (existingUser) {
      return res.status(400).json({ error: 'Username already exists' });
    }

    const hashedPassword = await hash(password, 10);
    const user = await db.users.createUser(username, hashedPassword);

    const token = jwt.sign({ username }, JWT_SECRET, { expiresIn: '24h' });
    res.json({ token, permissions: user.permissions });
  } catch (err) {
    console.error('Error registering user:', err);
    res.status(500).json({ error: 'Server error' });
  }
});

router.post('/login', credentialLimiter, async (req, res) => {
  const { username, password } = req.body;

  if (!username || !password) {
    return res.status(400).json({ error: 'Username and password required' });
  }

  try {
    const user = await db.users.getUserByUsername(username);
    if (!user) {
      return res.status(401).json({ error: 'Invalid credentials' });
    }

    const validPassword = await compare(password, user.password);
    if (!validPassword) {
      return res.status(401).json({ error: 'Invalid credentials' });
    }

    const token = jwt.sign({ username }, JWT_SECRET, { expiresIn: '24h' });
    res.json({ token, permissions: user.permissions });
  } catch (err) {
    console.error('Error logging in:', err);
    res.status(500).json({ error: 'Server error' });
  }
});

router.get('/state', authMiddleware, (req, res) => {
  res.json({
    authenticated: true,
    username: req.user!.username,
    permissions: req.user!.permissions
  });
});

// Unified endpoint to update user profile (username and/or password)
router.put('/profile', authMiddleware, async (req, res) => {
  const { newUsername, currentPassword, newPassword } = req.body;
  const currentUsername = req.user!.username;
  
  // Determine what we're updating
  const isUpdatingUsername = !!newUsername;
  const isUpdatingPassword = !!(currentPassword && newPassword);
  
  if (!isUpdatingUsername && !isUpdatingPassword) {
    return res.status(400).json({ 
      error: 'At least one of newUsername or newPassword (with currentPassword) must be provided' 
    });
  }
  
  try {
    // Get current user
    const currentUser = await db.users.getUserByUsername(currentUsername);
    if (!currentUser) {
      return res.status(404).json({ error: 'User not found' });
    }
    
    // Object to store updates
    const updates: Partial<User> = {};
    let updatedUsername = currentUsername;
    let needsNewToken = false;
    
    // Handle username update if requested
    if (isUpdatingUsername && newUsername !== currentUsername) {
      // Check if the new username already exists
      const existingUser = await db.users.getUserByUsername(newUsername);
      if (existingUser) {
        return res.status(400).json({ error: 'Username already taken' });
      }
      
      updates.username = newUsername;
      updatedUsername = newUsername;
      needsNewToken = true;
    }
    
    // Handle password update if requested
    if (isUpdatingPassword) {
      // Verify current password
      const validPassword = await compare(currentPassword, currentUser.password);
      if (!validPassword) {
        return res.status(401).json({ error: 'Current password is incorrect' });
      }
      
      // Hash new password
      updates.password = await hash(newPassword, 10);
    }
    
    // Update the user if we have changes
    if (Object.keys(updates).length > 0) {
      const updatedUser = await db.users.updateUser(
        { id: currentUser.id },
        updates
      );
      
      const response: any = { 
        message: 'Profile updated successfully',
        permissions: updatedUser.permissions
      };
      
      // Generate new token if username changed
      if (needsNewToken) {
        response.token = jwt.sign({ username: updatedUsername }, JWT_SECRET, { expiresIn: '24h' });
      }
      
      res.json(response);
    } else {
      res.json({ message: 'No changes made to profile' });
    }
  } catch (err) {
    console.error('Error updating user profile:', err);
    res.status(500).json({ error: 'Server error' });
  }
});

export default router;