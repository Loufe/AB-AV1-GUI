# src/ab_av1/exceptions.py
"""
Custom exceptions for ab-av1 related errors in the AV1 Video Converter application.
"""

class AbAv1Error(Exception):
    """Base exception for ab-av1 related errors"""
    def __init__(self, message, command=None, output=None, error_type=None):
        self.message = message
        self.command = command
        self.output = output
        self.error_type = error_type
        super().__init__(self.message)

class InputFileError(AbAv1Error): pass
class OutputFileError(AbAv1Error): pass
class VMAFError(AbAv1Error): pass
class EncodingError(AbAv1Error): pass


class ConversionNotWorthwhileError(Exception):
    """Exception raised when CRF search fails, indicating conversion isn't beneficial."""
    def __init__(self, message, command=None, output=None, original_size=None, estimated_size=None):
        super().__init__(message)
        self.error_type = "conversion_not_worthwhile"
        self.command = command
        self.output = output
        self.original_size = original_size
        self.estimated_size = estimated_size
